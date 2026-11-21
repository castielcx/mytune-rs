//! Crosshair overlay: a small transparent, click-through, always-on-top window with a crosshair at the exact
//! centre pixel of one monitor, toggled by a global hotkey. Own Win32 layered window on one thread that blocks
//! in `GetMessageW` (no polling, no repaint: the picture is drawn once per change). Works over borderless and
//! windowed games; exclusive fullscreen cannot be covered. Settings live in state.json under `Crosshair`.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const KEY: &str = "Crosshair";
/// Keys offered for the toggle hotkey (no modifiers, so it works while a game has focus).
pub const HOTKEYS: &[&str] = &["F6", "F7", "F8", "F9", "F10", "F11", "F12", "Insert", "Pause"];

/// Settings layout version: 0 = saved before the Lines / Center Dot / Outline editor (dot and outline took the
/// line colour and opacity, the dot was thickness + 2 wide); `clamped` migrates it.
const VERSION: u8 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crosshair {
    #[serde(rename = "On")]
    pub on: bool,
    #[serde(rename = "Hotkey")]
    pub hotkey: String,
    /// Display device name (`\\.\DISPLAY1`); empty = primary monitor.
    #[serde(rename = "Monitor")]
    pub monitor: String,
    #[serde(rename = "Ver", default)]
    pub ver: u8,
    // ---- lines
    #[serde(rename = "LinesOn")]
    pub lines_on: bool,
    /// Number of arms (1..8), evenly spaced, the first at 12 o'clock.
    #[serde(rename = "Arms")]
    pub arms: i32,
    /// Round ends instead of rectangles.
    #[serde(rename = "Round")]
    pub round: bool,
    /// Arm length in pixels (2..80).
    #[serde(rename = "Size")]
    pub size: i32,
    /// Line thickness in pixels (1..10).
    #[serde(rename = "Thickness")]
    pub thickness: i32,
    /// Empty space around the centre in pixels (0..40).
    #[serde(rename = "Gap")]
    pub gap: i32,
    /// Degrees clockwise, 0..359.
    #[serde(rename = "Rotation")]
    pub rotation: i32,
    /// Soft edge in pixels (0..8).
    #[serde(rename = "Blur")]
    pub blur: i32,
    #[serde(rename = "Color")]
    pub color: [u8; 3],
    /// Percent, 10..100.
    #[serde(rename = "Opacity")]
    pub opacity: i32,
    /// No first (upward) arm; the old "T shape".
    #[serde(rename = "TShape")]
    pub hide_top: bool,
    // ---- centre dot
    #[serde(rename = "Dot")]
    pub dot: bool,
    #[serde(rename = "DotColor")]
    pub dot_color: [u8; 3],
    #[serde(rename = "DotOpacity")]
    pub dot_opacity: i32,
    /// Width in pixels (1..40).
    #[serde(rename = "DotSize")]
    pub dot_size: i32,
    #[serde(rename = "DotCircle")]
    pub dot_circle: bool,
    #[serde(rename = "DotBlur")]
    pub dot_blur: i32,
    // ---- outline (around lines and dot)
    #[serde(rename = "Outline")]
    pub outline: bool,
    #[serde(rename = "OutlineColor")]
    pub outline_color: [u8; 3],
    #[serde(rename = "OutlineOpacity")]
    pub outline_opacity: i32,
    /// Pixels (1..5).
    #[serde(rename = "OutlineThickness")]
    pub outline_thickness: i32,
    #[serde(rename = "OutlineBlur")]
    pub outline_blur: i32,
}

impl Default for Crosshair {
    fn default() -> Self {
        Crosshair {
            on: false,
            hotkey: "F8".into(),
            monitor: String::new(),
            ver: VERSION,
            lines_on: true,
            arms: 4,
            round: false,
            size: 10,
            thickness: 2,
            gap: 4,
            rotation: 0,
            blur: 0,
            color: [0, 255, 0],
            opacity: 100,
            hide_top: false,
            dot: false,
            dot_color: [0, 255, 0],
            dot_opacity: 100,
            dot_size: 4,
            dot_circle: false,
            dot_blur: 0,
            outline: true,
            outline_color: [0, 0, 0],
            outline_opacity: 100,
            outline_thickness: 1,
            outline_blur: 0,
        }
    }
}

/// A named look; keeps the on/off state, hotkey and monitor.
pub const PRESETS: &[(&str, i32, i32, i32, [u8; 3], bool, bool)] = &[
    // name, size, thickness, gap, colour, dot, t_shape
    ("Classic", 10, 2, 4, [0, 255, 0], false, false),
    ("Small dot", 2, 3, 0, [255, 40, 40], true, false),
    ("Tight cross", 6, 1, 2, [255, 255, 255], false, false),
    ("Long cross", 18, 2, 6, [0, 230, 255], false, false),
    ("T shape", 10, 2, 3, [255, 220, 0], false, true),
];

impl Crosshair {
    /// A preset replaces the whole look (new options go back to their defaults).
    pub fn preset(&mut self, i: usize) {
        let (_, size, thickness, gap, color, dot, hide_top) = PRESETS[i];
        let look = Crosshair { size, thickness, gap, color, dot, hide_top, dot_color: color, dot_size: thickness + 2, ..Default::default() };
        self.apply_look(&look);
    }

    /// Takes the saved look's picture settings; keeps on/off, hotkey and monitor.
    pub fn apply_look(&mut self, o: &Crosshair) {
        let keep = (self.on, self.hotkey.clone(), self.monitor.clone());
        *self = Crosshair { on: keep.0, hotkey: keep.1, monitor: keep.2, ..o.clone() }.clamped();
    }

    /// Same picture: everything but on/off, hotkey and monitor.
    pub fn same_look(&self, o: &Crosshair) -> bool {
        let n = |c: &Crosshair| Crosshair { on: false, hotkey: String::new(), monitor: String::new(), ..c.clone() }.clamped();
        n(self) == n(o)
    }

    pub fn clamped(mut self) -> Self {
        if self.ver < VERSION {
            self.dot_color = self.color;
            self.dot_opacity = self.opacity;
            self.dot_size = self.thickness.clamp(1, 10) + 2;
            self.outline_opacity = self.opacity;
            self.ver = VERSION;
        }
        self.arms = self.arms.clamp(1, 8);
        self.size = self.size.clamp(2, 80);
        self.thickness = self.thickness.clamp(1, 10);
        self.gap = self.gap.clamp(0, 40);
        self.rotation = self.rotation.rem_euclid(360);
        self.blur = self.blur.clamp(0, 8);
        self.opacity = self.opacity.clamp(10, 100);
        self.dot_opacity = self.dot_opacity.clamp(10, 100);
        self.dot_size = self.dot_size.clamp(1, 40);
        self.dot_blur = self.dot_blur.clamp(0, 8);
        self.outline_opacity = self.outline_opacity.clamp(10, 100);
        self.outline_thickness = self.outline_thickness.clamp(1, 5);
        self.outline_blur = self.outline_blur.clamp(0, 8);
        // dot and lines share one centre only when both are odd or both even. Up for even, down for odd: stays in
        // 1..=40, and changing the thickness back and forth does not make the dot creep
        if self.lines_on && self.dot && (self.dot_size - self.thickness) % 2 != 0 {
            self.dot_size += if self.thickness % 2 == 0 { 1 } else { -1 };
        }
        if !HOTKEYS.contains(&self.hotkey.as_str()) {
            self.hotkey = "F8".into();
        }
        self
    }

    pub fn load(store: &Store) -> Crosshair {
        store.top(KEY).and_then(|v| serde_json::from_value::<Crosshair>(v.clone()).ok()).unwrap_or_default().clamped()
    }

    pub fn save(&self, store: &mut Store) -> crate::error::Result<()> {
        store.set_top(KEY, serde_json::to_value(self)?);
        store.save()
    }
}

/// A crosshair the user saved by name (state.json `CrosshairSaves`); a click applies it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Look")]
    pub look: Crosshair,
}

pub const SAVES_KEY: &str = "CrosshairSaves";

pub fn load_saves(store: &Store) -> Vec<Saved> {
    let mut v: Vec<Saved> = store.top(SAVES_KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    for s in &mut v {
        s.look = s.look.clone().clamped(); // migrates looks saved by older versions
    }
    v
}

pub fn save_saves(store: &mut Store, saves: &[Saved]) -> crate::error::Result<()> {
    store.set_top(SAVES_KEY, serde_json::to_value(saves)?);
    store.save()
}

// ---------------------------------------------------------------- rasteriser

/// Blur margin: three box passes of radius `rho` reach `3 * rho` pixels.
fn rho(b: i32) -> usize {
    (b as f32 * 0.6).ceil() as usize
}

/// Side of the (odd) square canvas whose middle pixel is the centre.
pub fn side(c: &Crosshair) -> i32 {
    let mut ext = 0;
    if c.lines_on {
        ext = c.gap + c.size + c.thickness + 2;
    }
    if c.dot {
        ext = ext.max(c.dot_size / 2 + 2);
    }
    let blur = [if c.lines_on { c.blur } else { 0 }, if c.dot { c.dot_blur } else { 0 }, if c.outline { c.outline_blur } else { 0 }];
    let margin = 3 * rho(blur.into_iter().max().unwrap_or(0)) as i32;
    2 * (ext + if c.outline { c.outline_thickness } else { 0 } + margin + 2) + 1
}

/// Distance from `p` to the segment a-b.
fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ab, ap) = ((b.0 - a.0, b.1 - a.1), (p.0 - a.0, p.1 - a.1));
    let l2 = ab.0 * ab.0 + ab.1 * ab.1;
    let t = if l2 > 0.0 { ((ap.0 * ab.0 + ap.1 * ab.1) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((ap.0 - ab.0 * t).hypot(ap.1 - ab.1 * t)).max(0.0)
}

/// Coverage of a pixel whose centre is `sd` away from the shape's edge (negative inside): crisp on whole pixels.
fn cover(sd: f32) -> f32 {
    let sd = (sd * 1024.0).round() / 1024.0; // float noise must not turn a crisp edge into 99 %
    (0.5 - sd).clamp(0.0, 1.0)
}

/// Box blur of radius `r` (window 2r+1, zero outside), applied three times: close to a gaussian, and symmetric.
fn blur(a: &mut [f32], s: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0f32; s];
    let k = 1.0 / (2 * r + 1) as f32;
    for pass in 0..6 {
        for line in 0..s {
            let at = |i: usize| if pass % 2 == 0 { line * s + i } else { i * s + line };
            for i in 0..s {
                tmp[i] = a[at(i)];
            }
            let mut sum: f32 = tmp[..(r + 1).min(s)].iter().sum();
            for i in 0..s {
                a[at(i)] = sum * k;
                if i + r + 1 < s {
                    sum += tmp[i + r + 1];
                }
                if i >= r {
                    sum -= tmp[i - r];
                }
            }
        }
    }
}

/// Premultiplied RGBA pixels, `side * side`, top-down, and the side. Pixel (side/2, side/2) is the centre.
/// Shapes are signed-distance fields sampled at pixel centres, so axis-aligned shapes on whole pixels stay crisp.
/// Order, bottom to top: outline ring (outside the shapes only), dot (where no line is), lines.
pub fn render_rgba(c: &Crosshair) -> (i32, Vec<u8>) {
    render_rgba_at(c, 1)
}

/// The same picture `z` times larger, as if each screen pixel were split into `z * z` samples: smooth edges stay
/// smooth instead of turning into blocks (the editor's enlarged preview). Side is `side(c) * z`.
pub fn render_rgba_at(c: &Crosshair, z: usize) -> (i32, Vec<u8>) {
    let z = z.max(1);
    let zf = z as f32;
    let s1 = side(c);
    let s = s1 * z as i32;
    let n = s as usize;
    let mid = (s1 / 2) as f32; // top-left corner of the centre pixel
    // an odd thickness is centred on the centre pixel, an even one on its top-left corner, so every arm is the
    // same distance from the centre and every edge lies on a pixel edge (crisp)
    let even = c.thickness % 2 == 0;
    let (centre, half) = (if even { (0.0f32, 0.0f32) } else { (0.5, 0.5) }, c.thickness as f32 / 2.0);
    let e = if even { 0.5 } else { 0.0 };
    let (r0, r1) = (c.gap as f32 - 0.5 + e, (c.gap + c.size) as f32 + 0.5 + e);
    let arms: Vec<(f32, f32, f32, f32)> = if c.lines_on {
        (0..c.arms)
            .filter(|i| !(c.hide_top && *i == 0))
            .map(|i| {
                let th = (c.rotation as f32 + i as f32 * 360.0 / c.arms as f32).to_radians();
                let (sn, cs) = th.sin_cos();
                // (origin x, origin y, dir x, dir y)
                (centre.0, centre.1, sn, -cs)
            })
            .collect()
    } else {
        vec![]
    };
    let dot_half = c.dot_size as f32 / 2.0;
    let dc = if c.dot_size % 2 == 1 { 0.5 } else { 0.0 };
    let (mut lines, mut dot, mut ring) = (vec![0f32; n * n], vec![0f32; n * n], vec![0f32; n * n]);
    let mut union_sd = vec![f32::MAX; n * n];
    // coverage of a sample whose distance (in screen pixels) is `sd`, at this zoom
    let cover = |sd: f32| if sd == f32::MAX { 0.0 } else { cover(sd * zf) };
    for y in 0..n {
        for x in 0..n {
            let p = ((x as f32 + 0.5) / zf - mid, (y as f32 + 0.5) / zf - mid);
            let i = y * n + x;
            let mut sd_l = f32::MAX;
            for &(ox, oy, dx, dy) in &arms {
                let q = (p.0 - ox, p.1 - oy);
                let (u, v) = (q.0 * dx + q.1 * dy, -q.0 * dy + q.1 * dx); // along the arm, across it
                let sd = if c.round {
                    let (a, b) = if r1 - r0 < 2.0 * half { ((r0 + r1) / 2.0, (r0 + r1) / 2.0) } else { (r0 + half, r1 - half) };
                    seg_dist((u, v), (a, 0.0), (b, 0.0)) - half
                } else {
                    let (cu, hu) = ((r0 + r1) / 2.0, (r1 - r0) / 2.0);
                    let e = ((u - cu).abs() - hu, v.abs() - half);
                    e.0.max(0.0).hypot(e.1.max(0.0)) + e.0.max(e.1).min(0.0)
                };
                sd_l = sd_l.min(sd);
            }
            let mut sd_d = f32::MAX;
            if c.dot {
                let q = (p.0 - dc, p.1 - dc);
                sd_d = if c.dot_circle {
                    q.0.hypot(q.1) - dot_half
                } else {
                    let e = (q.0.abs() - dot_half, q.1.abs() - dot_half);
                    e.0.max(0.0).hypot(e.1.max(0.0)) + e.0.max(e.1).min(0.0)
                };
            }
            lines[i] = cover(sd_l);
            dot[i] = cover(sd_d) * (1.0 - lines[i]);
            union_sd[i] = sd_l.min(sd_d);
        }
    }
    if c.outline {
        let o = c.outline_thickness as f32;
        for i in 0..n * n {
            ring[i] = cover(union_sd[i] - o) * (1.0 - cover(union_sd[i]));
        }
    }
    blur(&mut lines, n, rho(c.blur) * z);
    blur(&mut dot, n, rho(c.dot_blur) * z);
    blur(&mut ring, n, rho(c.outline_blur) * z);
    let layers = [
        (&ring, c.outline_color, c.outline_opacity, c.outline),
        (&dot, c.dot_color, c.dot_opacity, c.dot),
        (&lines, c.color, c.opacity, c.lines_on),
    ];
    let mut px = vec![0u8; n * n * 4];
    for i in 0..n * n {
        // premultiplied "over", bottom layer first
        let mut out = [0f32; 4];
        for (cov, rgb, op, on) in &layers {
            if !*on {
                continue;
            }
            let a = cov[i] * *op as f32 / 100.0;
            let k = 1.0 - a;
            out = [out[0] * k + rgb[0] as f32 / 255.0 * a, out[1] * k + rgb[1] as f32 / 255.0 * a, out[2] * k + rgb[2] as f32 / 255.0 * a, out[3] * k + a];
        }
        for ch in 0..4 {
            px[i * 4 + ch] = (out[ch] * 255.0 + 1e-3).min(255.0) as u8;
        }
    }
    (s, px)
}

/// The overlay's pixels: premultiplied BGRA, as `UpdateLayeredWindow` wants them.
pub fn render(c: &Crosshair) -> (i32, Vec<u8>) {
    let (s, mut px) = render_rgba(c);
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    (s, px)
}

// ---------------------------------------------------------------- window and hotkey thread

static LIVE: Mutex<Option<Crosshair>> = Mutex::new(None);
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);
static THREAD: AtomicU32 = AtomicU32::new(0);
type Hook = Box<dyn Fn(&Crosshair) + Send + Sync>;
static AFTER_TOGGLE: OnceLock<Hook> = OnceLock::new();

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
}

/// The current settings (the hotkey changes `on` behind the page's back).
pub fn live() -> Crosshair {
    LIVE.lock().unwrap_or_else(|p| p.into_inner()).clone().unwrap_or_default()
}

/// Why the hotkey is off, if it is.
pub fn problem() -> Option<String> {
    PROBLEM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Takes new settings: the overlay thread shows, moves, redraws or closes the window.
pub fn set(c: Crosshair) {
    *LIVE.lock().unwrap_or_else(|p| p.into_inner()) = Some(c.clamped());
    let t = THREAD.load(Ordering::Relaxed);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_APP, WPARAM(0), LPARAM(0));
        }
    }
}

/// Starts the overlay thread once. `after_toggle` runs on it after the hotkey flipped `on` (save, repaint).
pub fn start(initial: Crosshair, after_toggle: impl Fn(&Crosshair) + Send + Sync + 'static) {
    if AFTER_TOGGLE.set(Box::new(after_toggle)).is_err() {
        return;
    }
    *LIVE.lock().unwrap_or_else(|p| p.into_inner()) = Some(initial.clamped());
    std::thread::Builder::new().name("Crosshair".into()).spawn(run).ok();
}

fn vk_of(name: &str) -> Option<u32> {
    match name {
        "Insert" => Some(0x2D),
        "Pause" => Some(0x13),
        _ => name.strip_prefix('F')?.parse::<u32>().ok().filter(|n| (1..=12).contains(n)).map(|n| 0x6F + n),
    }
}

/// (left, top, width, height) of a display in pixels; the primary one when the device is empty or gone.
fn monitor_rect(device: &str) -> (i32, i32, i32, i32) {
    unsafe {
        if !device.is_empty() {
            let wide: Vec<u16> = device.encode_utf16().chain(Some(0)).collect();
            let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
            if EnumDisplaySettingsW(PCWSTR(wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() {
                let p = dm.Anonymous1.Anonymous2.dmPosition;
                return (p.x, p.y, dm.dmPelsWidth as i32, dm.dmPelsHeight as i32);
            }
        }
        (0, 0, GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
    }
}

unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

fn run() {
    unsafe {
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE); // creates this thread's queue
        THREAD.store(GetCurrentThreadId(), Ordering::Relaxed);
        let mut st = State { hwnd: None, vk: 0 };
        st.apply();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            match msg.message {
                WM_APP => st.apply(),
                WM_HOTKEY => {
                    let mut c = live();
                    c.on = !c.on;
                    set(c.clone());
                    st.apply();
                    if let Some(f) = AFTER_TOGGLE.get() {
                        f(&c);
                    }
                }
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

struct State {
    hwnd: Option<HWND>,
    vk: u32,
}

impl State {
    unsafe fn apply(&mut self) {
        let c = live();
        let vk = vk_of(&c.hotkey).unwrap_or(0x77);
        if vk != self.vk {
            unsafe {
                UnregisterHotKey(0, 1);
                if RegisterHotKey(0, 1, 0x4000, vk) == 0 {
                    let why = format!("{} is taken by another program, so the crosshair hotkey is off.", c.hotkey);
                    crate::log::info(&why);
                    *PROBLEM.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
                } else {
                    *PROBLEM.lock().unwrap_or_else(|p| p.into_inner()) = None;
                }
            }
            self.vk = vk;
        }
        unsafe {
            if !c.on {
                if let Some(h) = self.hwnd.take() {
                    let _ = DestroyWindow(h);
                }
                return;
            }
            let hwnd = match self.hwnd {
                Some(h) => h,
                None => match create() {
                    Some(h) => {
                        self.hwnd = Some(h);
                        h
                    }
                    None => return,
                },
            };
            draw(hwnd, &c);
        }
    }
}

unsafe fn create() -> Option<HWND> {
    unsafe {
        let inst = GetModuleHandleW(None).ok()?;
        let class = w!("MyTuneCrosshair");
        let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassExW(&wc); // fails harmlessly when it exists already
        // click-through, never activated, not on the taskbar
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
        CreateWindowExW(ex, class, w!("MyTune Crosshair"), WS_POPUP, 0, 0, 1, 1, None, None, Some(inst.into()), None).ok()
    }
}

unsafe fn draw(hwnd: HWND, c: &Crosshair) {
    unsafe {
        let (s, px) = render(c);
        let (l, t, w, h) = monitor_rect(&c.monitor);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: s, biHeight: -s, biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
            let old = SelectObject(mem, bmp.into());
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            // the canvas's middle pixel lands on the monitor's centre pixel
            let at = POINT { x: l + w / 2 - s / 2, y: t + h / 2 - s / 2 };
            let _ = UpdateLayeredWindow(hwnd, Some(screen), Some(&at), Some(&SIZE { cx: s, cy: s }), Some(mem), Some(&POINT::default()), COLORREF(0), Some(&blend), ULW_ALPHA);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare() -> Crosshair {
        Crosshair { outline: false, ..Default::default() }
    }

    fn pixel(s: i32, px: &[u8], x: i32, y: i32) -> u8 {
        px[(((y + s / 2) * s + x + s / 2) * 4 + 3) as usize]
    }

    #[test]
    fn odd_thickness_centres_on_the_centre_pixel() {
        let c = Crosshair { thickness: 3, gap: 0, size: 5, ..bare() };
        let (s, px) = render(&c);
        assert_eq!(s % 2, 1);
        for d in [-5, 0, 5] {
            assert_eq!(pixel(s, &px, d, 0), 255);
            assert_eq!(pixel(s, &px, 0, d), 255);
        }
        assert_eq!(pixel(s, &px, 0, 1), 255);
        assert_eq!(pixel(s, &px, 0, -1), 255);
        assert_eq!(pixel(s, &px, 6, 0), 0);
        assert_eq!(pixel(s, &px, 3, 3), 0);
        // crisp: every pixel is fully on or off
        assert!(px.chunks_exact(4).all(|p| p[3] == 0 || p[3] == 255));
    }

    #[test]
    fn odd_look_is_symmetric_under_quarter_turns() {
        let c = Crosshair { thickness: 3, gap: 2, size: 7, dot: true, dot_size: 5, ..Default::default() };
        let (s, px) = render_rgba(&c);
        let n = s as usize;
        let at = |x: usize, y: usize| &px[(y * n + x) * 4..(y * n + x) * 4 + 4];
        for y in 0..n {
            for x in 0..n {
                assert_eq!(at(x, y), at(n - 1 - y, x), "quarter turn at {x},{y}");
                assert_eq!(at(x, y), at(n - 1 - x, n - 1 - y), "half turn at {x},{y}");
            }
        }
        // a 90 degree rotation setting changes nothing for four arms
        let r = Crosshair { rotation: 90, ..c.clone() };
        assert_eq!(render_rgba(&r).1, px);
    }

    #[test]
    fn even_thickness_stays_crisp() {
        let (s, px) = render(&bare());
        assert!(px.chunks_exact(4).all(|p| p[3] == 0 || p[3] == 255));
        // thickness 2, gap 4, size 10, centred on the centre pixel's top-left corner:
        // right arm pixels 4..=14, left -15..=-5, both on rows -1 and 0
        for (x, on) in [(3, 0), (4, 255), (14, 255), (15, 0), (-4, 0), (-5, 255), (-15, 255), (-16, 0)] {
            assert_eq!((pixel(s, &px, x, -1), pixel(s, &px, x, 0)), (on, on), "x {x}");
        }
        assert_eq!((pixel(s, &px, -2, -5), pixel(s, &px, -1, -5), pixel(s, &px, 0, -5), pixel(s, &px, 1, -5)), (0, 255, 255, 0));
        assert_eq!(pixel(s, &px, 0, 0), 0);
    }

    #[test]
    fn even_look_is_symmetric_around_the_corner() {
        // the user saw thickness 2 one pixel longer on one side; mirror around the centre pixel's top-left corner
        for c in [Crosshair { dot: true, ..Default::default() }, Crosshair { thickness: 4, gap: 0, size: 6, ..Default::default() }] {
            let (s, px) = render_rgba(&c);
            let n = s as usize;
            let m = n / 2;
            let at = |x: usize, y: usize| &px[(y * n + x) * 4..(y * n + x) * 4 + 4];
            for y in 0..n - 1 {
                for x in 0..n - 1 {
                    assert_eq!(at(x, y), at(2 * m - 1 - y, x), "quarter turn at {x},{y}");
                    assert_eq!(at(x, y), at(2 * m - 1 - x, y), "mirror at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn dot_size_follows_line_parity_without_creeping() {
        let dot = |thickness, dot_size| Crosshair { thickness, dot_size, dot: true, ..Default::default() }.clamped().dot_size;
        assert_eq!((dot(2, 3), dot(2, 39), dot(3, 4), dot(3, 40), dot(1, 2)), (4, 40, 3, 39, 1));
        assert_eq!((dot(2, 4), dot(3, 5)), (4, 5));
        // 4 at an odd thickness is 3, and 3 at an even thickness is 4 again
        assert_eq!(dot(2, dot(3, 4)), 4);
        // without lines the dot keeps any size
        assert_eq!(Crosshair { lines_on: false, thickness: 2, dot: true, dot_size: 5, ..Default::default() }.clamped().dot_size, 5);
    }

    #[test]
    fn zoomed_picture_matches_and_is_smooth() {
        // crisp axis-aligned look (no outline: its corners are round): every zoomed block equals its screen pixel
        let c = bare();
        let (s, px) = render_rgba(&c);
        let (sz, pz) = render_rgba_at(&c, 4);
        assert_eq!(sz, s * 4);
        let (n, nz) = (s as usize, sz as usize);
        for y in 0..nz {
            for x in 0..nz {
                let (a, b) = ((y * nz + x) * 4, ((y / 4) * n + x / 4) * 4);
                assert_eq!(&pz[a..a + 4], &px[b..b + 4], "{x},{y}");
            }
        }
        // a round dot gets in-between alpha at the edge instead of blocks
        let d = Crosshair { lines_on: false, dot: true, dot_circle: true, dot_size: 9, ..bare() };
        let soft = |p: &[u8]| p.chunks_exact(4).filter(|q| q[3] > 0 && q[3] < 255).count();
        assert!(soft(&render_rgba_at(&d, 6).1) > soft(&render_rgba(&d).1) * 6);
    }

    #[test]
    fn gap_and_hide_top() {
        let c = Crosshair { thickness: 1, gap: 3, size: 4, hide_top: true, ..bare() };
        let (s, px) = render(&c);
        assert_eq!(pixel(s, &px, 0, 0), 0);
        assert_eq!(pixel(s, &px, 2, 0), 0);
        assert_eq!(pixel(s, &px, 3, 0), 255);
        assert_eq!(pixel(s, &px, -7, 0), 255);
        assert_eq!(pixel(s, &px, 0, 7), 255);
        assert_eq!(pixel(s, &px, 0, -3), 0); // no upward arm
    }

    #[test]
    fn outline_and_opacity() {
        let c = Crosshair { thickness: 1, gap: 0, size: 3, outline: true, opacity: 50, outline_opacity: 50, ..Default::default() };
        let (s, px) = render(&c);
        assert_eq!(pixel(s, &px, 0, 0), 127); // the outline is only drawn outside the lines
        assert_ne!(pixel(s, &px, 1, 1), 0);
        let i = (((1 + s / 2) * s + 1 + s / 2) * 4) as usize;
        assert_eq!(&px[i..i + 3], &[0, 0, 0]);
        // thicker outline reaches further
        let t = Crosshair { outline_thickness: 3, ..c };
        let (s, px) = render(&t);
        assert_ne!(pixel(s, &px, 0, 3), 0);
        assert_eq!(pixel(s, &px, 0, 8), 0);
    }

    #[test]
    fn blur_keeps_the_centre_symmetric() {
        let c = Crosshair { thickness: 3, blur: 4, dot: true, dot_size: 5, dot_blur: 3, outline_blur: 2, ..Default::default() };
        let (s, px) = render_rgba(&c);
        let n = s as usize;
        let at = |x: usize, y: usize| &px[(y * n + x) * 4..(y * n + x) * 4 + 4];
        for y in 0..n {
            for x in 0..n {
                let near = |a: &[u8], b: &[u8]| a.iter().zip(b).all(|(p, q)| p.abs_diff(*q) <= 1); // float order in the box sums
                assert!(near(at(x, y), at(n - 1 - x, n - 1 - y)), "half turn {x},{y}");
                assert!(near(at(x, y), at(n - 1 - y, x)), "quarter turn {x},{y}");
            }
        }
        let sharp = render_rgba(&Crosshair { blur: 0, ..c.clone() }).1;
        assert!(px.iter().zip(&sharp).any(|(a, b)| a != b));
        // soft edge: some pixel of the lines is partly transparent
        assert!(px.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));
    }

    #[test]
    fn arms_rotation_round_and_dot() {
        let c = Crosshair { arms: 1, size: 8, gap: 0, thickness: 3, ..bare() };
        let (s, px) = render(&c);
        assert_eq!(pixel(s, &px, 0, -8), 255); // one arm at 12 o'clock
        assert_eq!(pixel(s, &px, 0, 8), 0);
        let r = render(&Crosshair { rotation: 90, ..c.clone() });
        assert_eq!(pixel(r.0, &r.1, 8, 0), 255);
        let round = render(&Crosshair { round: true, ..c.clone() });
        assert!(pixel(round.0, &round.1, 1, -8) < 255); // the round tip is cut at the corner
        assert_eq!(pixel(round.0, &round.1, 0, -8), 255);
        let d = render(&Crosshair { lines_on: false, dot: true, dot_size: 7, dot_circle: true, ..bare() });
        assert_eq!(pixel(d.0, &d.1, 0, 0), 255);
        assert_eq!(pixel(d.0, &d.1, 3, 3), 0); // corner of a circle
        assert_eq!(pixel(d.0, &d.1, 3, 0), 255);
    }

    #[test]
    fn old_settings_load_with_the_same_look() {
        let old = r#"{"On":true,"Hotkey":"F9","Monitor":"","Size":12,"Thickness":3,"Gap":5,"Color":[255,40,40],"Opacity":60,"Dot":true,"Outline":true,"TShape":true}"#;
        let c: Crosshair = serde_json::from_str::<Crosshair>(old).unwrap().clamped();
        assert_eq!((c.color, c.dot_color, c.dot_opacity, c.dot_size, c.outline_opacity), ([255, 40, 40], [255, 40, 40], 60, 5, 60));
        assert!(c.dot && c.outline && c.hide_top && c.lines_on);
        assert_eq!((c.arms, c.rotation, c.blur), (4, 0, 0));
        // serialising again keeps the migration
        let again: Crosshair = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(again.clamped(), c);
        assert!(c.same_look(&Crosshair { on: false, hotkey: "F6".into(), ..c.clone() }));
        assert!(!c.same_look(&Crosshair { rotation: 10, ..c.clone() }));
    }

    #[test]
    fn json_round_trip_and_clamp() {
        let c = Crosshair { on: true, size: 500, arms: 99, hotkey: "Nope".into(), ..Default::default() };
        let v = serde_json::to_value(&c).unwrap();
        let back: Crosshair = serde_json::from_value(v).unwrap();
        let back = back.clamped();
        assert!(back.on);
        assert_eq!((back.size, back.arms, back.hotkey.as_str()), (80, 8, "F8"));
        let partial: Crosshair = serde_json::from_str(r#"{"On":true}"#).unwrap();
        assert_eq!(partial.thickness, 2);
        assert_eq!(vk_of("F8"), Some(0x77));
    }
}
