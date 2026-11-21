# Soundboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Discord-style soundboard popup in calls, with pressable sound tiles and keyboard shortcuts that work while a game has focus.

**Architecture:** The UI reads `AppState.voice.soundboard` and sends the engine's existing `voice::Command`s. Shortcuts live in `mytune_chat::Settings`. The existing low-level keyboard hook matches them and hands the sound id to a worker thread, which dispatches the play command.

**Tech Stack:** Rust, egui/eframe 0.37, `windows-sys`, Yapfast engine (`vendor-src`, not edited).

**Spec:** `docs/superpowers/specs/2026-11-21-soundboard-design.md`

## Global Constraints

- No edits under `vendor-src\`. No new crates. Do not update `Cargo.lock`.
- New buttons use `Sense::click_and_drag()` plus `theme::pull`. Text that may hold symbols uses `sb`, not `dsb`.
- Never call `ctx.input` inside `ctx.data_mut`. Chat textures go through `texture()` or `media::pic_tex`.
- Look comes from `theme.rs`: no gradients or glow.
- The hook procedure never blocks, never dispatches, and always calls `CallNextHookEx`.
- Hook runs only while push-to-talk is on, a key is being recorded, or a call has `voice.soundboard.is_some()` with at least one shortcut.
- Other agents edit this checkout: re-read before editing, stage only your own files, never restore files from git. Cargo commands other than the publish go to `target\agents` through the mod.

## Review Focus

- Saving the Settings page from an older copy must not wipe shortcuts (`set_settings` already guards `person_sound` the same way).
- Holding a shortcut key plays once. Releasing and pressing again plays again.
- `Ctrl+1` must not fire a shortcut bound to plain `1`, and the reverse.
- Pressing a bound key while recording a new shortcut records it and does not play.
- Leaving the call stops the hook unless push-to-talk still needs it. An old `chat-settings.json` without shortcuts still loads.

---

### Task 1: Shortcut storage and global key matching

**Files:**
- Modify: `crates/mytune-chat/src/settings.rs` (types, `Settings` field)
- Modify: `crates/mytune-chat/src/ptt.rs` (combo matching, recording, firing)
- Modify: `crates/mytune-chat/src/lib.rs` (`Inner`, `set_settings`, `watch_keys`, `changed`, new methods)

**Interfaces:**
- Produces in `settings.rs`, exported from the crate root:
  - `pub struct KeyCombo { pub ctrl: bool, pub shift: bool, pub alt: bool, pub scancode: u32, pub name: String }`, where `name` reads like `Ctrl+Shift+F5`.
  - `pub struct SoundShortcut { pub sound: String, pub name: String, pub emoji: String, pub combo: KeyCombo }`
  - `Settings.sound_shortcuts: Vec<SoundShortcut>`
- Produces in `ptt.rs`:
  - `pub fn set_shortcuts(list: Vec<(bool, bool, bool, u32, String)>)` (ctrl, shift, alt, scancode, sound id)
  - `pub fn set_fire(tx: Option<std::sync::mpsc::Sender<String>>)`
  - `pub fn record_combo(on: bool)`, `pub fn recording_combo() -> bool`, `pub fn recorded_combo() -> Option<KeyCombo>`
  - `fn find_shortcut<'a>(list: &'a [(bool, bool, bool, u32, String)], ctrl: bool, shift: bool, alt: bool, scancode: u32) -> Option<&'a str>`
  - `fn is_modifier(scancode: u32) -> bool` (Ctrl, Shift, Alt, Win, both sides)
- Produces on `Chat`:
  - `pub fn record_shortcut(&self, on: bool)`
  - `pub fn recorded_shortcut(&self) -> Option<KeyCombo>`
  - `pub fn set_shortcut(&self, shortcut: SoundShortcut)`: replaces any entry with the same sound or the same combo.
  - `pub fn remove_shortcut(&self, sound: &str)`

- [ ] **Step 1: Write failing unit tests in `ptt.rs` and `settings.rs`**

```rust
#[test]
fn combo_needs_exact_modifiers() {
    let list = vec![(true, false, false, 2, "a".to_string()), (false, false, false, 3, "b".to_string())];
    assert_eq!(find_shortcut(&list, true, false, false, 2), Some("a"));
    assert_eq!(find_shortcut(&list, false, false, false, 2), None);
    assert_eq!(find_shortcut(&list, true, false, false, 3), None);
}

#[test]
fn modifiers_alone_are_not_keys() {
    for code in [0x1D, 0xE01D, 0x2A, 0x36, 0x38, 0xE038, 0xE05B, 0xE05C] { assert!(is_modifier(code)); }
    assert!(!is_modifier(0x02));
}

#[test]
fn old_settings_have_no_shortcuts() {
    let s: Settings = serde_json::from_str("{}").unwrap();
    assert!(s.sound_shortcuts.is_empty());
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p mytune-chat combo_needs_exact_modifiers`
Expected: FAIL to compile, `find_shortcut` not found.

- [ ] **Step 3: Implement types, matching and hook changes**

Hook procedure, in order on key down: combo recording (skip modifiers, Esc cancels), then the existing push-to-talk recording and talk key, then shortcut lookup. Read modifiers with `GetAsyncKeyState`. Read the list with `try_read` and skip on contention. Ignore auto-repeat with a `HELD` atomic scan code, cleared on that key's key up. Send the id through `try_lock` on the sender.

- [ ] **Step 4: Wire `Chat`**

Start one `mytune-sound-keys` worker thread holding `Weak<Inner>`. It blocks on the channel, and for each id dispatches `Action::Voice(Command::Soundboard(id))` only when `voice.active()` and that id is in `voice.soundboard`. `set_settings` keeps `sound_shortcuts` from the current settings when `save` is true, as it does for `person_sound`. `watch_keys` also wants the hook for combo recording and for a soundboard call with shortcuts. `changed()` calls `watch_keys` only when that wanted state flips, outside the app lock.

- [ ] **Step 5: Run tests**

Run: `cargo test -p mytune-chat`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/mytune-chat/src/settings.rs crates/mytune-chat/src/ptt.rs crates/mytune-chat/src/lib.rs
git commit -m "Soundboard: global keyboard shortcuts"
```

### Task 2: Discord-style soundboard popup

**Files:**
- Create: `crates/mytune-ui/src/chatpage/soundboard.rs`
- Modify: `crates/mytune-ui/src/chatpage/voice.rs` (`CallUi`, `controls_row`, `dock`, stage width)
- Modify: `crates/mytune-ui/src/chatpage.rs` or its module root (`mod soundboard`, call `soundboard::show` beside `picker::show`)
- Test: `crates/mytune-ui/tests/shot_soundboard.rs`

**Interfaces:**
- Produces:
  - `#[derive(Default)] pub struct SoundboardUi` with `open`, `anchor: Option<egui::Pos2>`, `query`, `tab_shortcuts: bool`, `collapsed: HashSet<String>`, `fresh`, recording target.
  - `impl SoundboardUi { pub fn toggle(&mut self, anchor: egui::Pos2) }`
  - `pub fn show(ctx: &egui::Context, sb_ui: &mut SoundboardUi, chat: &mytune_chat::Chat, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>)`
  - `fn filter_groups(groups: &[SoundGroup], query: &str) -> Vec<SoundGroup>`
  - `fn blocked_reason(v: &voice::State) -> Option<&'static str>`, returning `"Undeafen to use the soundboard"` or `"You are server muted"`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn search_ignores_case_and_drops_empty_groups() {
    let g = vec![group("Server", &["Airhorn", "Bruh"]), group("Default", &["Cricket"])];
    let r = filter_groups(&g, "AIR");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].sounds[0].name, "Airhorn");
}

#[test]
fn deafened_blocks_playing() {
    let mut v = voice::State::default();
    v.deafened = true;
    assert_eq!(blocked_reason(&v), Some("Undeafen to use the soundboard"));
}
```

- [ ] **Step 2: Run and confirm failure**

Run: `cargo test -p mytune-ui --lib soundboard`
Expected: FAIL to compile.

- [ ] **Step 3: Build the popup**

Floating `egui::Area` in `Order::Foreground` using `theme::float`, about 480 x 520, above its button and clamped to the screen. Search on top, 44 px server rail on the left that jumps to a section, collapsible section headers, three-column pill tiles with emoji or custom emoji picture and name. Tile click sends `Vc::Soundboard`. Tile hover shows preview (`Vc::PreviewSound`) and star (`Vc::FavoriteSound`). Greyed tiles show `blocked_reason`. `v.warning` shows at the bottom, which is where the builder must confirm a Discord refusal arrives. Click outside or Esc closes.

- [ ] **Step 4: Add the button**

Round button right after Share in `controls_row`, and dock button after Share in `dock`, each only when `v.soundboard.is_some()`. Update `n_round` and the dock's width divisor for the extra button. Use a music-note or soundboard glyph from the icon font.

- [ ] **Step 5: Add a headless screenshot test and view it**

Run: `SHOT_DIR=<scratchpad> cargo test -p mytune-ui --test shot_soundboard`
Expected: PASS, and the PNG shows search, rail, sections and tiles with fake groups.

- [ ] **Step 6: Commit**

```bash
git add crates/mytune-ui/src/chatpage/soundboard.rs crates/mytune-ui/src/chatpage/voice.rs crates/mytune-ui/tests/shot_soundboard.rs
git commit -m "Soundboard: Discord-style popup in calls"
```

Also stage the module root file changed in Step 3 by its path.

### Task 3: Shortcuts tab, docs, publish

**Files:**
- Modify: `crates/mytune-ui/src/chatpage/soundboard.rs`
- Modify: `crates/mytune-ui/tests/shot_soundboard.rs`
- Modify: `docs/features.md`

**Interfaces:**
- Consumes: `Chat::record_shortcut`, `recorded_shortcut`, `set_shortcut`, `remove_shortcut`, `SoundShortcut`, `KeyCombo` from Task 1. `SoundboardUi` from Task 2.

- [ ] **Step 1: Build the tab**

Header tabs `Sounds` and `Shortcuts`. Each row: emoji, sound name, key chip, remove button. "Add shortcut" opens a sound list, then records a key. Tile right-click offers "Set shortcut". While recording, the chip reads `Press a key…`, polled with `request_repaint_after` only during recording, and Esc cancels. A combo already used moves to the new sound, and the row says so.

- [ ] **Step 2: Extend the screenshot test with the Shortcuts tab and view it**

Run: `SHOT_DIR=<scratchpad> cargo test -p mytune-ui --test shot_soundboard`
Expected: PASS, and the PNG shows rows with key chips.

- [ ] **Step 3: Add a `docs/features.md` entry**

What was built, code locations, what was measured, and that a real play against Discord and in-game shortcuts were not tried.

- [ ] **Step 4: Commit**

```bash
git add crates/mytune-ui/src/chatpage/soundboard.rs crates/mytune-ui/tests/shot_soundboard.rs docs/features.md
git commit -m "Soundboard: keyboard shortcuts tab"
```

- [ ] **Step 5: Publish**

Run alone in PowerShell with timeout 600000: `& 'C:\Users\domin\Documents\MyTune\mytune-rs\target\.agents\publish.ps1'`
Expected: build succeeds and MyTune restarts minimized, or the answer is "already live". When refused, retry after 40 s.
