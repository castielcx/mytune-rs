//! `cust.selectioncolor`: the colour of the drag-selection border in Explorer and on the desktop. That border is
//! the Windows system colour Hilight, so this sets `HKCU\Control Panel\Colors\Hilight` (for the next sign-in) and
//! `SetSysColors` (for now). The original of both is in the tweak's backup; the choice is `Settings.SelectionColor`.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::reg::{self, Hive, RegValue};
use crate::store::Store;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{COLORREF, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{GetSysColor, SetSysColors, COLOR_HIGHLIGHT};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};

pub const ID: &str = "cust.selectioncolor";
const PATH: &str = "Control Panel\\Colors";
const NAME: &str = "Hilight";
const SETTING: &str = "SelectionColor";
/// 1: the desktop draws the colored drag box instead of the old dotted one.
const ADVANCED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced";
const ALPHA: &str = "ListviewAlphaSelect";
/// Backup key of the live colour: the registry value can differ from it, or be missing.
const LIVE: &str = "live";

pub type Rgb = [u8; 3];

pub const PALETTE: [Rgb; 24] = [
    [200, 30, 30], [239, 68, 68], [249, 115, 22], [245, 158, 11], [234, 179, 8], [163, 230, 53],
    [34, 197, 94], [16, 185, 129], [20, 184, 166], [6, 182, 212], [14, 165, 233], [0, 120, 215],
    [37, 99, 235], [79, 70, 229], [124, 58, 237], [168, 85, 247], [217, 70, 239], [236, 72, 153],
    [244, 63, 94], [146, 64, 14], [255, 255, 255], [156, 163, 175], [75, 85, 99], [0, 0, 0],
];

static CHOSEN: Mutex<Option<Rgb>> = Mutex::new(None);
static ON: AtomicBool = AtomicBool::new(false);

fn chosen() -> Option<Rgb> {
    *CHOSEN.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn parse(s: &str) -> Option<Rgb> {
    let mut parts = s.split_whitespace().map(|p| p.parse::<u8>().ok());
    let rgb = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(rgb)
}

pub fn format(c: Rgb) -> String {
    format!("{} {} {}", c[0], c[1], c[2])
}

/// The palette with `current` in it: a colour that is not in the palette takes the last cell.
pub fn palette_with(current: Rgb) -> [Rgb; 24] {
    let mut cells = PALETTE;
    if !cells.contains(&current) {
        cells[23] = current;
    }
    cells
}

fn live() -> Rgb {
    let [r, g, b, _] = unsafe { GetSysColor(COLOR_HIGHLIGHT) }.to_le_bytes();
    [r, g, b]
}

fn set_live(c: Rgb) -> Result<()> {
    let color = COLORREF(u32::from_le_bytes([c[0], c[1], c[2], 0]));
    unsafe { SetSysColors(1, &COLOR_HIGHLIGHT.0, &color) }.map_err(|e| Error::Plain(format!("Windows refused the color: {e}")))
}

fn in_registry() -> Result<Option<Rgb>> {
    Ok(match reg::get(Hive::Hkcu, PATH, NAME)? {
        Some(RegValue::String(s)) => parse(&s),
        _ => None,
    })
}

/// At start, before the first state read: the saved choice, and on while a backup exists.
pub fn load(store: &Store) {
    *CHOSEN.lock().unwrap_or_else(|p| p.into_inner()) = store.settings_str(SETTING).and_then(|s| parse(&s));
    ON.store(store.get_backup(ID).is_some(), Ordering::Relaxed);
}

/// The colour to show as selected: the choice, else what Windows has now.
pub fn current() -> Rgb {
    chosen().or_else(|| in_registry().ok().flatten()).unwrap_or_else(live)
}

/// Saves the choice. Changes nothing in Windows: while the tweak is on, the caller applies it again.
pub fn choose(store: &mut Store, c: Rgb) -> Result<()> {
    store.set_settings_str(SETTING, &format(c))?;
    *CHOSEN.lock().unwrap_or_else(|p| p.into_inner()) = Some(c);
    Ok(())
}

/// While on, the taskbar thread keeps the hook that removes the desktop box's fill.
pub(crate) fn is_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// A backup from before `ListviewAlphaSelect` was part of this tweak. A partly applied one is left for Restore.
fn needs_upgrade(b: &TweakBackup) -> bool {
    !b.partly_applied && !b.registry.iter().any(|r| r.name.eq_ignore_ascii_case(ALPHA))
}

/// Once at start, off the UI thread: an older install that is on gains the desktop box. The normal apply saves the
/// still untouched `ListviewAlphaSelect` into the existing backup before writing it; the Hilight original stays.
pub fn upgrade(engine: &crate::engine::Engine) {
    let old = engine.with_store(|s| s.get_backup(ID).is_some_and(needs_upgrade));
    if old && SelectionColorTweak.is_applied().unwrap_or(false) {
        if let Err(e) = engine.apply(&SelectionColorTweak) {
            crate::log::error("Selection color: could not add the desktop box", Some(&e));
        }
    }
}

/// Tells Explorer the Advanced settings changed. Bounded: hung windows are skipped, the others get 500 ms each.
fn notify_shell() {
    let text: Vec<u16> = "ShellState".encode_utf16().chain(Some(0)).collect();
    for lp in [text.as_ptr() as isize, 0] {
        unsafe {
            let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(lp), SMTO_ABORTIFHUNG, 500, None);
        }
    }
}

pub struct SelectionColorTweak;

impl Tweak for SelectionColorTweak {
    fn id(&self) -> &str {
        ID
    }
    fn title(&self) -> &str {
        "Selection border color"
    }
    fn is_applied(&self) -> Result<bool> {
        if !ON.load(Ordering::Relaxed) {
            return Ok(false);
        }
        Ok(chosen().is_none() || in_registry()? == chosen())
    }
    // first capture only: changing the color while on keeps the true original
    fn capture(&self, backup: &mut TweakBackup) -> Result<()> {
        backup.capture(Hive::Hkcu, PATH, NAME)?;
        backup.capture(Hive::Hkcu, ADVANCED, ALPHA)?;
        backup.data.entry(LIVE.into()).or_insert_with(|| format(live()));
        Ok(())
    }
    fn apply(&self, backup: &mut TweakBackup) -> Result<()> {
        self.capture(backup)?;
        let c = current();
        reg::set(Hive::Hkcu, PATH, NAME, &RegValue::String(format(c)))?;
        set_live(c)?;
        reg::set(Hive::Hkcu, ADVANCED, ALPHA, &RegValue::Dword(1))?;
        ON.store(true, Ordering::Relaxed);
        notify_shell();
        Ok(())
    }
    fn revert(&self, backup: &TweakBackup) -> Result<()> {
        let restored = backup.restore_registry();
        notify_shell(); // also after a partial restore: what was put back should show
        restored?;
        if let Some(c) = backup.data.get(LIVE).and_then(|s| parse(s)) {
            set_live(c)?;
        }
        ON.store(false, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_rgb() {
        assert_eq!(parse("200 30 30"), Some([200, 30, 30]));
        assert_eq!(parse("  0   120 215 "), Some([0, 120, 215]));
        assert_eq!(format([200, 30, 30]), "200 30 30");
        for bad in ["", "1 2", "1 2 3 4", "256 0 0", "-1 0 0", "a b c"] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_a_clean_backup_without_the_alpha_value_is_upgraded() {
        let entry = |name: &str| crate::backup::RegBackup { hive: "HKCU".into(), path: "p".into(), name: name.into(), existed: false, kind: None, value: None, multi: None, created_keys: vec![] };
        let mut b = TweakBackup { registry: vec![entry(NAME)], ..Default::default() };
        assert!(needs_upgrade(&b));
        b.partly_applied = true;
        assert!(!needs_upgrade(&b));
        b.partly_applied = false;
        b.registry.push(entry(ALPHA));
        assert!(!needs_upgrade(&b));
    }

    #[test]
    fn palette_always_holds_the_current_color() {
        assert_eq!(palette_with([0, 120, 215]), PALETTE);
        let cells = palette_with([1, 2, 3]);
        assert_eq!(cells[23], [1, 2, 3]);
        assert_eq!(cells[..23], PALETTE[..23]);
        assert_eq!(PALETTE.iter().collect::<std::collections::HashSet<_>>().len(), 24);
    }
}
