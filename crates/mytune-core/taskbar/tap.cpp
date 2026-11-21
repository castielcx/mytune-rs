// MyTune taskbar look. Windows' XAML diagnostics (InitializeXamlDiagnosticsEx, called by
// mytune-core/src/taskbar.rs) loads this DLL into explorer.exe. It finds the taskbar's background
// rectangle and swaps its brush, and can hide Windows' own volume icon in the tray. MyTune says what to
// do through the shared memory `Local\MyTuneTaskbar10`; when MyTune stops writing its heartbeat, the
// normal taskbar comes back.
#include <unknwn.h>
#include <windows.h>
#include <ocidl.h>
#include <psapi.h>
#pragma comment(lib, "gdi32.lib")
#pragma comment(lib, "kernel32.lib")
#undef GetCurrentTime // windows.h macro, breaks a XAML header
#include <xamlOM.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.UI.h>
#include <winrt/Windows.UI.Xaml.h>
#include <winrt/Windows.UI.Xaml.Controls.h>
#include <winrt/Windows.UI.Xaml.Hosting.h>
#include <windows.ui.xaml.hosting.desktopwindowxamlsource.h>
#include <winrt/Windows.UI.Xaml.Media.h>
#include <winrt/Windows.UI.Xaml.Shapes.h>
#include <bit>
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
    volatile LONG max_look; // look and colour of a taskbar on a monitor listed in `max_mons`
    volatile LONG max_argb;
    volatile LONG max_count;
    volatile LONG hide_volume; // 1: Windows' volume icon in the tray is hidden
    volatile LONG64 max_mons[8]; // HMONITORs with a maximised window
    volatile LONG64 start_mon;   // HMONITOR of the open Start menu / Search flyout, 0 = none
    volatile LONG start_look;    // look and colour of the taskbar on `start_mon` only
    volatile LONG start_argb;
    volatile LONG outline_select; // 1: the drag-selection box has no fill
    volatile LONG active;         // 1: a look or hide_volume is on, so `tick` runs every 100 ms
};
static Shared* g_shared = nullptr;
static volatile LONG g_no_fill = 0; // `outline_select` while the heartbeat is live, set by `tick`

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
    HWND island = nullptr; // the taskbar window it sits in, found from `t_sources`
};
struct Line {
    InstanceHandle handle;
    UIElement element;
    double opacity = 1;  // what it had before this DLL hid it
    bool hidden = false; // hidden by this DLL
};

// The plate behind a taskbar button. Its border is light at the top: on a clear or dark taskbar
// that shows as a bar above the active app's icon.
struct Plate {
    InstanceHandle handle;
    Controls::Border border;
    bool hidden = false; // border removed by this DLL
};

// A button of the tray's system icons (SystemTray.OmniButton*, SystemTray.AccentButton): network,
// volume, battery, clock. Only the volume item inside it is hidden (see `volume_icon`).
struct TrayButton {
    InstanceHandle handle;
    UIElement element;
    UIElement icon{nullptr};                    // the volume item hidden inside it
    Visibility original = Visibility::Visible; // what the item had before this DLL hid it
    bool hidden = false;                        // hidden by this DLL
};

// Everything below is per XAML thread: XAML objects must stay on the thread that made them.
thread_local std::vector<TrayButton> t_buttons;
thread_local std::vector<Plate> t_plates;
thread_local std::vector<Bar> t_bars;
thread_local std::vector<Line> t_lines;
struct Source {
    InstanceHandle handle;
    Hosting::DesktopWindowXamlSource source;
};
thread_local std::vector<Source> t_sources; // one per taskbar (monitor)
thread_local DispatcherTimer t_timer{nullptr};
thread_local Media::Brush t_brush{nullptr}, t_max_brush{nullptr}, t_start_brush{nullptr};
thread_local LONG t_look = 0, t_argb = 0, t_beat = 0, t_max_look = 0, t_max_argb = 0, t_start_look = 0, t_start_argb = 0;
thread_local ULONGLONG t_beat_at = 0; // GetTickCount64 of the last heartbeat change, 0 = none seen
thread_local bool t_fast = false;     // the timer runs at 100 ms

static Media::Brush make_brush(LONG argb) {
    return Media::SolidColorBrush(Windows::UI::Color{(uint8_t)(argb >> 24), (uint8_t)(argb >> 16), (uint8_t)(argb >> 8), (uint8_t)argb});
}

static HWND island_of(const Bar& b) {
    auto root = b.shape.XamlRoot();
    if (!root) return nullptr;
    for (auto& [h, s] : t_sources) {
        auto content = s.Content();
        if (!content || content.XamlRoot() != root) continue;
        HWND w = nullptr;
        if (auto native = s.try_as<IDesktopWindowXamlSourceNative>()) native->get_WindowHandle(&w);
        return w;
    }
    return nullptr;
}

// This bar sits on the monitor `mon`. A bar whose window is not known yet is not matched.
static bool on_monitor(Bar& b, LONG64 mon) {
    if (!b.island || !IsWindow(b.island)) b.island = island_of(b);
    return b.island && (LONG64)(INT_PTR)MonitorFromWindow(b.island, MONITOR_DEFAULTTONEAREST) == mon;
}

// A maximised window on this bar's monitor. A bar whose window is not known (yet) counts any monitor.
static bool on_max_monitor(Bar& b, LONG count) {
    if (count <= 0) return false;
    if (!b.island || !IsWindow(b.island)) b.island = island_of(b);
    if (!b.island) return true;
    auto mon = (LONG64)(INT_PTR)MonitorFromWindow(b.island, MONITOR_DEFAULTTONEAREST);
    for (LONG i = 0; i < count && i < 8; i++)
        if (g_shared->max_mons[i] == mon) return true;
    return false;
}

// Speaker glyphs of Segoe Fluent Icons: mute E74F, levels E992-E995, faint bars EBC5 (measured on
// build 26200 with UI Automation: the volume button draws E994 over EBC5; network draws E839).
static bool volume_glyph(wchar_t c) {
    return c == 0xE74F || (c >= 0xE992 && c <= 0xE995) || c == 0xEBC5;
}

static bool shows_volume(DependencyObject o, int depth = 0) {
    if (auto t = o.try_as<Controls::TextBlock>()) {
        auto text = t.Text();
        return text.size() == 1 && volume_glyph(text[0]);
    }
    if (depth > 12) return false;
    int n = Media::VisualTreeHelper::GetChildrenCount(o);
    for (int i = 0; i < n; i++)
        if (shows_volume(Media::VisualTreeHelper::GetChild(o, i), depth + 1)) return true;
    return false;
}

// The volume icon inside a tray button. On build 26200 network and volume are two SystemTray.IconView
// items of one SystemTray.OmniButton (logged 2026-11-21), so only the item is collapsed: its
// ContentPresenter container, else the IconView itself.
static UIElement volume_icon(DependencyObject o, int depth = 0) {
    if (get_class_name(o) == L"SystemTray.IconView") {
        if (!shows_volume(o)) return nullptr;
        auto parent = Media::VisualTreeHelper::GetParent(o);
        if (auto cp = parent ? parent.try_as<Controls::ContentPresenter>() : nullptr) return cp;
        return o.try_as<UIElement>();
    }
    if (depth > 10) return nullptr;
    int n = Media::VisualTreeHelper::GetChildrenCount(o);
    for (int i = 0; i < n; i++)
        if (auto found = volume_icon(Media::VisualTreeHelper::GetChild(o, i), depth + 1)) return found;
    return nullptr;
}

static void patch_selection_fill();

static void tick() try {
    if (!g_shared) return;
    LONG beat = g_shared->beat;
    ULONGLONG now = GetTickCount64();
    if (beat != t_beat) {
        t_beat = beat;
        t_beat_at = now;
    }
    bool stale = !t_beat_at || now - t_beat_at >= 3000; // 3 s without a heartbeat: MyTune is gone
    bool fast = !stale && g_shared->active;
    if (fast != t_fast) {
        t_fast = fast;
        t_timer.Interval(std::chrono::milliseconds(fast ? 100 : 1000));
    }
    g_no_fill = !stale && g_shared->outline_select;
    if (g_no_fill) patch_selection_fill();
    LONG base = stale ? 0 : (LONG)g_shared->look, argb = g_shared->argb;
    LONG max_look = stale ? 0 : (LONG)g_shared->max_look, max_argb = g_shared->max_argb;
    LONG count = stale ? 0 : (LONG)g_shared->max_count;
    LONG64 start_mon = stale ? 0 : (LONG64)g_shared->start_mon;
    LONG start_look = start_mon ? (LONG)g_shared->start_look : 0, start_argb = g_shared->start_argb;
    if (start_look != t_start_look || start_argb != t_start_argb) {
        t_start_look = start_look;
        t_start_argb = start_argb;
        t_start_brush = start_look ? make_brush(start_argb) : nullptr;
    }
    if (base != t_look || argb != t_argb) {
        t_look = base;
        t_argb = argb;
        t_brush = base ? make_brush(argb) : nullptr;
    }
    if (max_look != t_max_look || max_argb != t_max_argb) {
        t_max_look = max_look;
        t_max_argb = max_argb;
        t_max_brush = max_look ? make_brush(max_argb) : nullptr;
    }
    // Lines and plates follow "some taskbar has a look": they are not matched to a monitor.
    LONG look = base || (count > 0 && max_look) || start_look;
    for (auto& b : t_bars) {
        // The Start menu / Search look is for the taskbar of their own monitor only.
        bool st = start_look && on_monitor(b, start_mon);
        bool max = !st && on_max_monitor(b, count);
        LONG look = st ? start_look : max ? max_look : base;
        auto& brush = st ? t_start_brush : max ? t_max_brush : t_brush;
        auto now = b.shape.Fill();
        if (!look) {
            if (b.ours && now == b.ours) b.shape.Fill(b.original);
            b.ours = nullptr;
        } else if (now != brush) {
            // The taskbar sets its own brush on theme changes: that one becomes the one to restore.
            if (!b.ours || now != b.ours) b.original = now;
            b.shape.Fill(brush);
            b.ours = brush;
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
    for (auto& p : t_plates) {
        if (look) {
            auto t = p.border.BorderThickness();
            if (t.Left || t.Top || t.Right || t.Bottom) {
                p.border.BorderThickness({});
                p.hidden = true;
            }
        } else if (p.hidden) {
            p.border.ClearValue(Controls::Border::BorderThicknessProperty());
            p.hidden = false;
        }
    }
    bool hide_volume = !stale && g_shared->hide_volume;
    for (auto& b : t_buttons) {
        // An item the tray recycled for another icon, or took out of the tree, is given back and looked up again.
        if (b.hidden && (!Media::VisualTreeHelper::GetParent(b.icon) || !shows_volume(b.icon))) {
            b.icon.Visibility(b.original);
            b.hidden = false;
            b.icon = nullptr;
        }
        if (hide_volume) {
            if (!b.hidden) {
                b.icon = volume_icon(b.element);
                if (!b.icon) continue; // its glyph may come later through a binding
                b.original = b.icon.Visibility();
                b.hidden = true;
                log(L"volume icon hidden");
            }
            // The tray sets it visible again on its own (device change, rebuild): hide it again.
            if (b.icon.Visibility() != Visibility::Collapsed) b.icon.Visibility(Visibility::Collapsed);
        } else if (b.hidden) {
            b.icon.Visibility(b.original);
            b.hidden = false;
            b.icon = nullptr;
        }
    }
} catch (...) {
    log(L"tick failed");
}

static void start_timer() {
    if (t_timer) return;
    t_timer = DispatcherTimer();
    t_timer.Interval(std::chrono::milliseconds(1000));
    t_timer.Tick([](auto&&, auto&&) { tick(); });
    t_timer.Start();
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
            std::erase_if(t_plates, [&](auto& p) { return p.handle == e.Handle; });
            std::erase_if(t_sources, [&](auto& s) { return s.handle == e.Handle; });
            std::erase_if(t_buttons, [&](auto& b) { return b.handle == e.Handle; });
            return S_OK;
        }
        std::wstring_view type = e.Type ? e.Type : L"";
        if (type.starts_with(L"SystemTray.OmniButton") || type == L"SystemTray.AccentButton") {
            com_ptr<::IInspectable> raw;
            if (SUCCEEDED(diag->GetIInspectableFromHandle(e.Handle, raw.put())))
                if (auto el = raw.try_as<UIElement>()) {
                    log(L"found ", e.Type);
                    t_buttons.push_back({e.Handle, el});
                    start_timer();
                }
            return S_OK;
        }
        if (e.Type && std::wstring_view(e.Type) == winrt::name_of<Hosting::DesktopWindowXamlSource>()) {
            com_ptr<::IInspectable> raw;
            if (SUCCEEDED(diag->GetIInspectableFromHandle(e.Handle, raw.put())))
                if (auto s = raw.try_as<Hosting::DesktopWindowXamlSource>()) t_sources.push_back({e.Handle, s});
            return S_OK;
        }
        std::wstring_view name = e.Name ? e.Name : L"";
        bool fill = name == L"BackgroundFill", stroke = name == L"BackgroundStroke", plate = name == L"BackgroundElement";
        if (!fill && !stroke && !plate) return S_OK;
        com_ptr<::IInspectable> raw;
        if (FAILED(diag->GetIInspectableFromHandle(e.Handle, raw.put()))) return S_OK;
        if (plate) {
            // Taskbar.TaskListButtonPanel or Taskbar.TaskListLabeledButtonPanel
            auto border = raw.try_as<Controls::Border>();
            auto parent = border ? Media::VisualTreeHelper::GetParent(border) : nullptr;
            if (!parent || std::wstring_view(get_class_name(parent)).rfind(L"Taskbar.TaskList", 0) != 0) return S_OK;
            log(L"found plate");
            t_plates.push_back({e.Handle, border});
        } else {
            auto shape = raw.try_as<Shapes::Shape>();
            if (!shape || !in_taskbar(shape)) return S_OK;
            log(L"found ", e.Name);
            if (fill) {
                t_bars.push_back({e.Handle, shape});
                if (g_shared) InterlockedIncrement(&g_shared->bars);
            }
            else t_lines.push_back({e.Handle, shape});
        }
        start_timer();
        return S_OK;
    } catch (...) {
        log(L"OnVisualTreeChange failed");
        return S_OK;
    }
    HRESULT __stdcall OnElementStateChanged(InstanceHandle, VisualElementState, LPCWSTR) noexcept override { return S_OK; }
};

// Outline-only selection, list views. With ListviewAlphaSelect=1, comctl32 (6.0.26100.8972, 0x1800c9244) paints
// the drag box as a 1x1 DIB of alpha 0x46 stretched by GdiAlphaBlend over the part being repainted, then draws
// its own 1 px COLOR_HIGHLIGHT frame. So only the fill is skipped; a frame drawn here would follow the repaint
// rectangle and leave lines inside the box.
static decltype(&GdiAlphaBlend) g_real_blend = nullptr;

static bool is_selection_fill(HDC src) {
    BITMAP bm;
    HGDIOBJ h = GetCurrentObject(src, OBJ_BITMAP);
    return h && GetObjectW(h, sizeof bm, &bm) == sizeof bm && bm.bmWidth == 1 && bm.bmHeight == 1 && bm.bmBitsPixel == 32 &&
           bm.bmBits && static_cast<const BYTE*>(bm.bmBits)[3] == 0x46;
}

static BOOL WINAPI selection_blend(HDC dest, int x, int y, int w, int h, HDC src, int sx, int sy, int sw, int sh, BLENDFUNCTION blend) {
    if (g_no_fill && sw == 1 && sh == 1 && std::bit_cast<DWORD>(blend) == 0x01FF0000 && is_selection_fill(src)) return TRUE;
    return g_real_blend(dest, x, y, w, h, src, sx, sy, sw, sh, blend);
}

// Points this comctl32's import of GdiAlphaBlend at `selection_blend`. Any surprise leaves it as it was.
static void patch_blend_import(HMODULE module) {
    auto base = reinterpret_cast<BYTE*>(module);
    auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return;
    auto nt = reinterpret_cast<IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
    if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC) return;
    DWORD imports = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress;
    if (!imports) return;
    for (auto d = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base + imports); d->Name; d++) {
        if (!d->OriginalFirstThunk) continue;
        auto name = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + d->OriginalFirstThunk);
        auto slot = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + d->FirstThunk);
        for (; name->u1.AddressOfData; name++, slot++) {
            if (IMAGE_SNAP_BY_ORDINAL64(name->u1.Ordinal)) continue;
            auto by_name = reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + name->u1.AddressOfData);
            if (strcmp(by_name->Name, "GdiAlphaBlend")) continue;
            auto target = reinterpret_cast<void**>(&slot->u1.Function);
            auto current = reinterpret_cast<decltype(&GdiAlphaBlend)>(*target);
            if (current == selection_blend) return;
            if (!g_real_blend) g_real_blend = current;
            else if (current != g_real_blend) return; // another hook here: one saved original cannot serve both
            DWORD old;
            if (VirtualProtect(target, sizeof(void*), PAGE_READWRITE, &old)) {
                InterlockedExchangePointer(target, reinterpret_cast<void*>(selection_blend));
                VirtualProtect(target, sizeof(void*), old, &old);
            }
            return;
        }
    }
}

// Outline-only selection, folder windows. ExplorerFrame makes one DirectUI `UIMarqueeSelector` (id `Marquee`)
// per view, when the view opens. Element::Paint fills its Background (style value `hotlight`) and takes the border colour from
// BorderColor, so a transparent local Background removes the fill only. COLOR_HOTLIGHT itself stays untouched.
static const char k_create_element[] = "?CreateElement@DUIXmlParser@DirectUI@@QEAAJPEBGPEAVElement@2@1PEAKPEAPEAV32@@Z";
using CreateElementFn = HRESULT(__fastcall*)(void*, const wchar_t*, void*, void*, unsigned long*, void**);
using SetBackgroundFn = HRESULT(__fastcall*)(void*, DWORD);
static CreateElementFn g_real_create = nullptr;
static SetBackgroundFn g_set_background = nullptr;
static volatile LONG g_marquee_done = 0;

static HRESULT __fastcall marquee_create(void* parser, const wchar_t* id, void* a, void* b, unsigned long* c, void** out) {
    HRESULT hr = g_real_create(parser, id, a, b, c, out);
    if (SUCCEEDED(hr) && g_no_fill && out && *out && !IS_INTRESOURCE(id) && !wcscmp(id, L"Marquee")) g_set_background(*out, 0);
    return hr;
}

static void** find_import(BYTE* base, DWORD names_rva, DWORD slots_rva) {
    auto name = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + names_rva);
    auto slot = reinterpret_cast<IMAGE_THUNK_DATA64*>(base + slots_rva);
    for (; name->u1.AddressOfData; name++, slot++)
        if (!IMAGE_SNAP_BY_ORDINAL64(name->u1.Ordinal) &&
            !strcmp(reinterpret_cast<IMAGE_IMPORT_BY_NAME*>(base + name->u1.AddressOfData)->Name, k_create_element))
            return reinterpret_cast<void**>(&slot->u1.Function);
    return nullptr;
}

// Delay-loaded in this Windows build; a normal import is handled too. Null: not found or unreadable.
static void** find_create_slot(BYTE* base, SIZE_T* image_size) {
    __try {
        auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
        if (dos->e_magic != IMAGE_DOS_SIGNATURE) return nullptr;
        auto nt = reinterpret_cast<IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
        if (nt->Signature != IMAGE_NT_SIGNATURE || nt->OptionalHeader.Magic != IMAGE_NT_OPTIONAL_HDR64_MAGIC) return nullptr;
        *image_size = nt->OptionalHeader.SizeOfImage;
        if (DWORD rva = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT].VirtualAddress)
            for (auto d = reinterpret_cast<IMAGE_DELAYLOAD_DESCRIPTOR*>(base + rva); d->DllNameRVA; d++)
                if (d->Attributes.RvaBased && d->ImportNameTableRVA)
                    if (auto slot = find_import(base, d->ImportNameTableRVA, d->ImportAddressTableRVA)) return slot;
        if (DWORD rva = nt->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT].VirtualAddress)
            for (auto d = reinterpret_cast<IMAGE_IMPORT_DESCRIPTOR*>(base + rva); d->Name; d++)
                if (d->OriginalFirstThunk)
                    if (auto slot = find_import(base, d->OriginalFirstThunk, d->FirstThunk)) return slot;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
    }
    return nullptr;
}

// Called from `tick` until done, at most once a second. A view makes its Marquee once, when it opens, so both
// DLLs are loaded here: waiting for the first folder window would leave that window's box filled.
static void patch_marquee_create() {
    static ULONGLONG last = 0;
    if (g_marquee_done) return;
    ULONGLONG now = GetTickCount64();
    if (last && now - last < 1000) return;
    last = now;
    HMODULE frame = LoadLibraryExW(L"explorerframe.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    HMODULE dui = LoadLibraryExW(L"dui70.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
    if (!frame || !dui) return;
    auto create = reinterpret_cast<void*>(GetProcAddress(dui, k_create_element));
    auto set = reinterpret_cast<void*>(GetProcAddress(dui, "?SetBackgroundColor@Element@DirectUI@@QEAAJK@Z"));
    auto base = reinterpret_cast<BYTE*>(frame);
    SIZE_T size = 0;
    void** slot = create && set ? find_create_slot(base, &size) : nullptr;
    if (!slot) {
        g_marquee_done = 1; // this Windows build lacks the import or export: leave it alone
        return;
    }
    void* hook = reinterpret_cast<void*>(marquee_create);
    void* current = *slot;
    // An unbound delay slot points at ExplorerFrame's own stub; any other value is someone else's hook and stays.
    bool unbound = static_cast<BYTE*>(current) >= base && static_cast<BYTE*>(current) < base + size;
    if (current == hook || (current != create && !unbound)) {
        g_marquee_done = 1;
        return;
    }
    g_real_create = reinterpret_cast<CreateElementFn>(create);
    g_set_background = reinterpret_cast<SetBackgroundFn>(set);
    DWORD old;
    if (!VirtualProtect(slot, sizeof(void*), PAGE_READWRITE, &old)) return;
    // shortcut: a delay-load bind racing this write can put the real import back; the fill then stays until Explorer restarts.
    if (InterlockedCompareExchangePointer(slot, hook, current) == current) g_marquee_done = 1;
    VirtualProtect(slot, sizeof(void*), old, &old);
}

static void patch_selection_fill() {
    static LONG once = 0;
    static volatile LONG pinned = 0;
    if (!InterlockedExchange(&once, 1)) {
        // The patched imports will point into this DLL: it must never be unmapped.
        HMODULE self;
        if (GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, reinterpret_cast<LPCWSTR>(&selection_blend), &self)) {
            pinned = 1;
            HMODULE modules[1024];
            DWORD bytes = 0;
            if (K32EnumProcessModules(GetCurrentProcess(), modules, sizeof modules, &bytes)) {
                DWORD count = (bytes > sizeof modules ? sizeof modules : bytes) / sizeof(HMODULE);
                for (DWORD i = 0; i < count; i++) {
                    wchar_t path[MAX_PATH];
                    DWORD n = GetModuleFileNameW(modules[i], path, MAX_PATH);
                    if (!n || n >= MAX_PATH || _wcslwr_s(path)) continue;
                    if (wcsstr(path, L"common-controls_6595b64144ccf1df_6.") && wcsstr(path, L"\\comctl32.dll")) patch_blend_import(modules[i]);
                }
            }
        }
    }
    if (pinned) patch_marquee_create();
}

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
        HANDLE m = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0, sizeof(Shared), L"Local\\MyTuneTaskbar10");
        if (m) g_shared = static_cast<Shared*>(MapViewOfFile(m, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared)));
        log(g_shared ? L"loaded" : L"no shared memory");
    }
    return make_self<Factory>()->QueryInterface(iid, out);
} catch (...) {
    return E_FAIL;
}

// Never unloads: XAML objects and the timer point into this DLL.
STDAPI DllCanUnloadNow() { return S_FALSE; }
