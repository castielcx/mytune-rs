// MyTune clear Start menu. Windows' XAML diagnostics (InitializeXamlDiagnosticsEx, called by
// mytune-core/src/taskbar.rs) loads this DLL into StartMenuExperienceHost.exe and into SearchHost.exe
// (the panel that takes over when a query is typed). It makes the panel's backgrounds transparent,
// leaves the panel's border and gives clickable items a darker plate. MyTune switches it through the shared memory
// `Local\MyTuneStart1.<process id>`; when MyTune stops writing its heartbeat, the normal Start menu
// comes back. The process id is in the name because SearchHost is an app container whose names fall
// back to the session's: with one name both processes ended up on the same memory.
#include <unknwn.h>
#include <windows.h>
#include <ocidl.h>
#undef GetCurrentTime // windows.h macro, breaks a XAML header
#include <xamlOM.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <chrono>
#include <string_view>
#include <vector>

using namespace winrt;
using namespace Windows::UI::Xaml;

// Weak dark tint on the outer panel only (inner plates stay clear, so nothing stacks): black at this
// alpha leaves a white window behind at about 255 * (1 - A/255), still clearly see-through.
static constexpr BYTE TINT_ALPHA = 0x50;
// Stronger backing plate, same black, for XAML controls that carry text (the search boxes). It sits on
// top of the panel tint, so the two stack: about 255 * (1 - 0x50/255) * (1 - 0x90/255) over white.
static constexpr BYTE PLATE_ALPHA = 0x90;

// The search results are a web page (WebView2 in SearchHost). Its chips, result rows, group headers and
// the detail pane's action rows get the same plate through a <style id="mytune-plate"> that `tick` puts
// into the page once a second while on (a reloaded page gets it back) and takes out when off. Our own
// background-color overrides the page's hover colour, so hover, keyboard selection and focus get a white
// layer on top of the plate. Dark theme only: in the light theme the text is dark.
static constexpr wchar_t ADD_PLATES[] =
    LR"((function(){var d=document,s=d.getElementById('mytune-plate');if(!s){s=d.createElement('style');s.id='mytune-plate';(d.head||d.documentElement).appendChild(s);}var c=`)"
    LR"(.darkTheme :is(.scope-tile:not(.scope-tile--selected) .scope-tile__title,li.suggestion,.groupHeader,li.sectionItem){background-color:rgba(0,0,0,.565)!important})"
    LR"(.darkTheme :is(.scope-tile:not(.scope-tile--selected):hover .scope-tile__title,.scope-tile:not(.scope-tile--selected):focus-visible .scope-tile__title,li.suggestion:hover,li.suggestion.selected,.groupHeader:hover,.groupHeader.selected,li.sectionItem:hover,li.sectionItem.selected){background-image:linear-gradient(rgba(255,255,255,.1),rgba(255,255,255,.1))!important})"
    LR"(`;if(s.textContent!==c)s.textContent=c;})())";
static constexpr wchar_t REMOVE_PLATES[] = LR"((function(){var s=document.getElementById('mytune-plate');if(s)s.remove();})())";

// WebView2Standalone.Controls.IWebView2 (WebView2Standalone.winmd in the CBS package): after IInspectable
// come get_CoreWebView2, get_CoreWebView2Controller, EnsureCoreWebView2Async, then ExecuteScriptAsync (slot 9).
struct __declspec(uuid("59C47E46-CC96-525F-A17C-2C213F988447")) IWebView2Abi : ::IInspectable {
    virtual HRESULT __stdcall get_CoreWebView2(void**) = 0;
    virtual HRESULT __stdcall get_CoreWebView2Controller(void**) = 0;
    virtual HRESULT __stdcall EnsureCoreWebView2Async(void**) = 0;
    virtual HRESULT __stdcall ExecuteScriptAsync(HSTRING script, void** operation) = 0;
};

// Must match `StartShared` in taskbar.rs.
struct Shared {
    volatile LONG beat;   // MyTune counts this up while it runs
    volatile LONG on;     // 1 = clear Start menu
    volatile LONG panels; // Start panels found, so MyTune can tell when Windows renamed them
};
static Shared* g_shared = nullptr;

// {7A3B5C1E-9D2F-4E6A-B8C4-1F0E2D3C4B5C}, also in taskbar.rs.
static const CLSID CLSID_Tap = {0x7a3b5c1e, 0x9d2f, 0x4e6a, {0xb8, 0xc4, 0x1f, 0x0e, 0x2d, 0x3c, 0x4b, 0x5c}};

// A border whose background this DLL clears. Their borders stay.
// Start: the panel (`AcrylicBorder`), the plate behind the pinned apps (`AcrylicOverlay`) and the
// search box (`BorderElement`).
// Search: the panel (`AppBorder`, `AccentAppBorder`), its plate (`LayerBorder`, `AccentLayerBorder`)
// and the search box (`TaskbarSearchBackground`). The results under them are a web page (WebView2),
// styled through ADD_PLATES instead. The footer's user and power buttons (`StartDocked.NavigationPaneButton`)
// are XAML controls and get the stronger plate as their Background; their pointer-over state draws its own.
struct Plate {
    InstanceHandle handle;
    Controls::Border border;
    Media::Brush original{nullptr};
    Media::Brush ours{nullptr};
    bool panel = false;
    bool box = false; // a search box: gets the stronger plate
    Controls::Control button{nullptr}; // set instead of `border` for a footer button
    Media::Brush get() const { return button ? button.Background() : border.Background(); }
    void set(Media::Brush const& b) const { button ? button.Background(b) : border.Background(b); }
};

// Per XAML thread: XAML objects must stay on the thread that made them.
thread_local std::vector<Plate> t_plates;
thread_local DispatcherTimer t_timer{nullptr};
thread_local Media::Brush t_clear{nullptr};
thread_local Media::Brush t_tint{nullptr};
thread_local Media::Brush t_plate{nullptr};
thread_local LONG t_beat = 0;
thread_local int t_stale = 1000;
thread_local std::vector<std::pair<InstanceHandle, DependencyObject>> t_web_hosts; // HostedWebView2Control
thread_local int t_web_tick = 10;
thread_local int t_unstyle = 0; // ticks left that send REMOVE_PLATES (a busy page may miss one)

// Fire and forget: the page runs the script on its own; a page that is not ready yet just fails.
static void run_script(IWebView2Abi* view, wchar_t const* script) {
    hstring text{script};
    void* operation = nullptr;
    if (SUCCEEDED(view->ExecuteScriptAsync(static_cast<HSTRING>(get_abi(text)), &operation)) && operation)
        static_cast<::IUnknown*>(operation)->Release();
}

// The WebView2 sits a few levels under its host and reports no tree event of its own.
static com_ptr<IWebView2Abi> find_view(DependencyObject const& o, int depth = 0) {
    com_ptr<IWebView2Abi> view;
    if (SUCCEEDED(static_cast<::IUnknown*>(get_abi(o))->QueryInterface(__uuidof(IWebView2Abi), view.put_void()))) return view;
    if (depth < 4) {
        int n = Media::VisualTreeHelper::GetChildrenCount(o);
        for (int i = 0; i < n; i++)
            if (auto found = find_view(Media::VisualTreeHelper::GetChild(o, i), depth + 1)) return found;
    }
    return nullptr;
}

static void run_in_pages(wchar_t const* script) {
    for (auto& [handle, host] : t_web_hosts)
        if (auto view = find_view(host)) run_script(view.get(), script);
}

static void tick() try {
    if (!g_shared) return;
    LONG beat = g_shared->beat;
    bool was_stale = t_stale >= 30;
    if (beat != t_beat) {
        t_beat = beat;
        t_stale = 0;
    } else if (t_stale < 1000) {
        t_stale++;
    }
    bool stale = t_stale >= 30; // 3 s without a heartbeat: MyTune is gone
    if (stale != was_stale) t_timer.Interval(std::chrono::milliseconds(stale ? 1000 : 100));
    bool on = !stale && g_shared->on;
    if (on && !t_clear) {
        t_clear = Media::SolidColorBrush(Windows::UI::Color{0, 255, 255, 255});
        t_tint = Media::SolidColorBrush(Windows::UI::Color{TINT_ALPHA, 0, 0, 0});
        t_plate = Media::SolidColorBrush(Windows::UI::Color{PLATE_ALPHA, 0, 0, 0});
    }
    for (auto& p : t_plates) {
        auto now = p.get();
        auto want = p.panel ? t_tint : p.box ? t_plate : t_clear;
        if (!on) {
            // Only what this DLL changed is put back.
            if (p.ours && now == p.ours) p.set(p.original);
            p.ours = nullptr;
        } else if (now != want) {
            // Start sets its own brush on theme and state changes: that one becomes the one to restore.
            if (!p.ours || now != p.ours) p.original = now;
            p.set(want);
            p.ours = want;
        }
    }
    if (on) {
        t_unstyle = 3;
        if (++t_web_tick >= 10) {
            t_web_tick = 0;
            run_in_pages(ADD_PLATES);
        }
    } else if (t_unstyle > 0) {
        t_unstyle--;
        t_web_tick = 10; // the first tick after switching on styles at once
        run_in_pages(REMOVE_PLATES);
    }
} catch (...) {
}

static void start_timer() {
    if (t_timer) return;
    t_timer = DispatcherTimer();
    t_timer.Interval(std::chrono::milliseconds(1000));
    t_timer.Tick([](auto&&, auto&&) { tick(); });
    t_timer.Start();
}

static bool in_search_box(DependencyObject o) {
    for (int i = 0; i < 8 && o; i++) {
        o = Media::VisualTreeHelper::GetParent(o);
        if (o && get_class_name(o) == L"StartMenu.SearchBoxToggleButton") return true;
    }
    return false;
}

struct Tap : implements<Tap, IObjectWithSite, IVisualTreeServiceCallback2, non_agile> {
    com_ptr<IUnknown> site;
    com_ptr<IXamlDiagnostics> diag;

    HRESULT __stdcall SetSite(IUnknown* s) noexcept override {
        site.copy_from(s);
        diag = site ? site.try_as<IXamlDiagnostics>() : nullptr;
        if (!diag) return S_OK;
        // AdviseVisualTreeChange hangs when called from this thread.
        AddRef();
        HANDLE t = CreateThread(nullptr, 0, [](void* p) -> DWORD {
            auto self = static_cast<Tap*>(p);
            if (auto tree = self->diag.try_as<IVisualTreeService3>()) tree->AdviseVisualTreeChange(self);
            self->Release();
            return 0;
        }, this, 0, nullptr);
        if (t) CloseHandle(t); else Release();
        return S_OK;
    }
    HRESULT __stdcall GetSite(REFIID iid, void** out) noexcept override {
        return site ? site->QueryInterface(iid, out) : E_FAIL;
    }

    HRESULT __stdcall OnVisualTreeChange(ParentChildRelation, VisualElement e, VisualMutationType type) noexcept override try {
        if (type == Remove) {
            auto gone = std::erase_if(t_plates, [&](auto& p) { return p.handle == e.Handle && p.panel; });
            if (g_shared && gone) InterlockedAdd(&g_shared->panels, -(LONG)gone);
            std::erase_if(t_plates, [&](auto& p) { return p.handle == e.Handle; });
            std::erase_if(t_web_hosts, [&](auto& w) { return w.first == e.Handle; });
            return S_OK;
        }
        if (e.Type && std::wstring_view(e.Type) == L"Cortana.UI.Views.HostedWebView2Control") {
            com_ptr<::IInspectable> raw;
            if (SUCCEEDED(diag->GetIInspectableFromHandle(e.Handle, raw.put()))) {
                if (auto host = raw.try_as<DependencyObject>()) {
                    t_web_hosts.emplace_back(e.Handle, host);
                    start_timer();
                }
            }
            return S_OK;
        }
        if (e.Type && std::wstring_view(e.Type) == L"StartDocked.NavigationPaneButton") {
            com_ptr<::IInspectable> raw;
            if (SUCCEEDED(diag->GetIInspectableFromHandle(e.Handle, raw.put()))) {
                if (auto button = raw.try_as<Controls::Control>()) {
                    t_plates.push_back({e.Handle, nullptr, nullptr, nullptr, false, true, button});
                    start_timer();
                }
            }
            return S_OK;
        }
        std::wstring_view name = e.Name ? e.Name : L"";
        bool panel = name == L"AcrylicBorder" || name == L"AppBorder", box = name == L"BorderElement";
        if (!panel && !box && name != L"AcrylicOverlay" && name != L"AccentAppBorder" && name != L"LayerBorder" &&
            name != L"AccentLayerBorder" && name != L"TaskbarSearchBackground")
            return S_OK;
        com_ptr<::IInspectable> raw;
        if (FAILED(diag->GetIInspectableFromHandle(e.Handle, raw.put()))) return S_OK;
        auto border = raw.try_as<Controls::Border>();
        if (!border || (box && !in_search_box(border))) return S_OK;
        t_plates.push_back({e.Handle, border, nullptr, nullptr, panel, box || name == L"TaskbarSearchBackground"});
        if (panel && g_shared) InterlockedIncrement(&g_shared->panels);
        start_timer();
        return S_OK;
    } catch (...) {
        return S_OK;
    }
    HRESULT __stdcall OnElementStateChanged(InstanceHandle, VisualElementState, LPCWSTR) noexcept override { return S_OK; }
};

struct Factory : implements<Factory, IClassFactory> {
    HRESULT __stdcall CreateInstance(IUnknown* outer, REFIID iid, void** out) noexcept override try {
        if (outer) return CLASS_E_NOAGGREGATION;
        return make_self<Tap>()->QueryInterface(iid, out);
    } catch (...) {
        return E_FAIL;
    }
    HRESULT __stdcall LockServer(BOOL) noexcept override { return S_OK; }
};

#pragma comment(linker, "/export:DllGetClassObject,PRIVATE")
#pragma comment(linker, "/export:DllCanUnloadNow,PRIVATE")

STDAPI DllGetClassObject(REFCLSID clsid, REFIID iid, void** out) try {
    if (clsid != CLSID_Tap) return CLASS_E_CLASSNOTAVAILABLE;
    if (!g_shared) {
        // Made here, not by MyTune: MyTune runs elevated and this process could not open its objects.
        wchar_t name[64];
        swprintf_s(name, L"Local\\MyTuneStart1.%lu", GetCurrentProcessId());
        HANDLE m = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0, sizeof(Shared), name);
        if (m) g_shared = static_cast<Shared*>(MapViewOfFile(m, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared)));
    }
    return make_self<Factory>()->QueryInterface(iid, out);
} catch (...) {
    return E_FAIL;
}

// Never unloads: XAML objects and the timer point into this DLL.
STDAPI DllCanUnloadNow() { return S_FALSE; }
