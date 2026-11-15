// MyTune taskbar look. Windows' XAML diagnostics (InitializeXamlDiagnosticsEx, called by
// mytune-core/src/taskbar.rs) loads this DLL into explorer.exe. It finds the taskbar's background
// rectangle and swaps its brush. MyTune says which look through the shared memory `Local\MyTuneTaskbar1`;
// when MyTune stops writing its heartbeat, the normal taskbar comes back.
#include <unknwn.h>
#include <windows.h>
#include <ocidl.h>
#undef GetCurrentTime // windows.h macro, breaks a XAML header
#include <xamlOM.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Windows.UI.Xaml.Shapes.h>
#include <chrono>
#include <string_view>
#include <vector>

using namespace winrt;
using namespace Windows::UI::Xaml;

// Must match `Shared` in taskbar.rs.
struct Shared {
    volatile LONG beat; // MyTune counts this up while it runs
    volatile LONG look; // 0 normal, 1 colour (alpha 0 = clear)
    volatile LONG argb;
    volatile LONG bars; // taskbar backgrounds found, so MyTune can tell when Windows renamed them
};
static Shared* g_shared = nullptr;

// {7A3B5C1E-9D2F-4E6A-B8C4-1F0E2D3C4B5A}, also in taskbar.rs.
static const CLSID CLSID_Tap = {0x7a3b5c1e, 0x9d2f, 0x4e6a, {0xb8, 0xc4, 0x1f, 0x0e, 0x2d, 0x3c, 0x4b, 0x5a}};

#ifdef TAP_LOG
static void log(const wchar_t* text, const wchar_t* more = L"") {
    wchar_t path[MAX_PATH];
    GetTempPathW(MAX_PATH, path);
    wcscat_s(path, L"mytune-taskbar.log");
    HANDLE f = CreateFileW(path, FILE_APPEND_DATA, FILE_SHARE_READ, nullptr, OPEN_ALWAYS, 0, nullptr);
    if (f == INVALID_HANDLE_VALUE) return;
    DWORD n;
    WriteFile(f, text, (DWORD)(wcslen(text) * 2), &n, nullptr);
    WriteFile(f, more, (DWORD)(wcslen(more) * 2), &n, nullptr);
    WriteFile(f, L"\r\n", 4, &n, nullptr);
    CloseHandle(f);
}
#else
#define log(...)
#endif

struct Bar {
    InstanceHandle handle;
    Shapes::Shape shape;
    Media::Brush original{nullptr};
    Media::Brush ours{nullptr};
};
struct Line {
    InstanceHandle handle;
    UIElement element;
    double opacity = 1;  // what it had before this DLL hid it
    bool hidden = false; // hidden by this DLL
};

// Everything below is per XAML thread: XAML objects must stay on the thread that made them.
thread_local std::vector<Bar> t_bars;
thread_local std::vector<Line> t_lines;
thread_local DispatcherTimer t_timer{nullptr};
thread_local Media::Brush t_brush{nullptr};
thread_local LONG t_look = 0, t_argb = 0, t_beat = 0;
thread_local int t_stale = 1000;

static Media::Brush make_brush(LONG argb) {
    return Media::SolidColorBrush(Windows::UI::Color{(uint8_t)(argb >> 24), (uint8_t)(argb >> 16), (uint8_t)(argb >> 8), (uint8_t)argb});
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
    LONG look = stale ? 0 : (LONG)g_shared->look, argb = g_shared->argb;
    if (look != t_look || argb != t_argb) {
        t_look = look;
        t_argb = argb;
        t_brush = look ? make_brush(argb) : nullptr;
    }
    for (auto& b : t_bars) {
        auto now = b.shape.Fill();
        if (!look) {
            if (b.ours && now == b.ours) b.shape.Fill(b.original);
            b.ours = nullptr;
        } else if (now != t_brush) {
            // The taskbar sets its own brush on theme changes: that one becomes the one to restore.
            if (!b.ours || now != b.ours) b.original = now;
            b.shape.Fill(t_brush);
            b.ours = t_brush;
        }
    }
    // Only what this DLL changed is put back: an idle copy (older version, MyTune closed) must not
    // undo the work of the active one. It did once: the line blinked every second.
    for (auto& l : t_lines) {
        if (look) {
            if (!l.hidden) {
                l.opacity = l.element.Opacity();
                l.hidden = true;
            }
            if (l.element.Opacity() != 0) l.element.Opacity(0);
        } else if (l.hidden) {
            l.element.Opacity(l.opacity);
            l.hidden = false;
        }
    }
} catch (...) {
    log(L"tick failed");
}

static bool in_taskbar(DependencyObject o) {
    for (int i = 0; i < 8 && o; i++) {
        o = Media::VisualTreeHelper::GetParent(o);
        if (o && get_class_name(o) == L"Taskbar.TaskbarBackground") return true;
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
            if (auto tree = self->diag.try_as<IVisualTreeService3>()) {
                HRESULT hr = tree->AdviseVisualTreeChange(self);
                if (FAILED(hr)) log(L"AdviseVisualTreeChange failed");
            }
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
            auto gone = std::erase_if(t_bars, [&](auto& b) { return b.handle == e.Handle; });
            if (g_shared && gone) InterlockedAdd(&g_shared->bars, -(LONG)gone);
            std::erase_if(t_lines, [&](auto& l) { return l.handle == e.Handle; });
            return S_OK;
        }
        std::wstring_view name = e.Name ? e.Name : L"";
        bool fill = name == L"BackgroundFill", stroke = name == L"BackgroundStroke";
        if (!fill && !stroke) return S_OK;
        com_ptr<::IInspectable> raw;
        if (FAILED(diag->GetIInspectableFromHandle(e.Handle, raw.put()))) return S_OK;
        auto shape = raw.try_as<Shapes::Shape>();
        if (!shape || !in_taskbar(shape)) return S_OK;
        log(L"found ", e.Name);
        if (fill) {
            t_bars.push_back({e.Handle, shape});
            if (g_shared) InterlockedIncrement(&g_shared->bars);
        }
        else t_lines.push_back({e.Handle, shape});
        if (!t_timer) {
            t_timer = DispatcherTimer();
            t_timer.Interval(std::chrono::milliseconds(1000));
            t_timer.Tick([](auto&&, auto&&) { tick(); });
            t_timer.Start();
        }
        return S_OK;
    } catch (...) {
        log(L"OnVisualTreeChange failed");
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
        // Made here, not by MyTune: MyTune runs elevated and explorer could not open its objects.
        HANDLE m = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0, sizeof(Shared), L"Local\\MyTuneTaskbar1");
        if (m) g_shared = static_cast<Shared*>(MapViewOfFile(m, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared)));
        log(g_shared ? L"loaded" : L"no shared memory");
    }
    return make_self<Factory>()->QueryInterface(iid, out);
} catch (...) {
    return E_FAIL;
}

// Never unloads: XAML objects and the timer point into this DLL.
STDAPI DllCanUnloadNow() { return S_FALSE; }
