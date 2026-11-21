# Forced One-Click Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Public MyTune installs check GitHub at start and daily and, from the next start, stay locked on an update screen until a signed new release is installed with one click.

**Architecture:** `mytune-core::updater` owns the signed manifest check, the update state, the background thread, the download and the setup start, all behind `cfg(feature = "license")`. `mytune-ui::updategate` draws the screen and the title-bar banner; `App::ui` puts the gate before the licence gate. `mytune-keys` signs each setup, and the Inno script relaunches MyTune after an update or a failed update.

**Tech Stack:** Rust, egui/eframe 0.37, ureq 2, ed25519-dalek 2, sha2 0.10, base64 0.22, Inno Setup 6, PowerShell.

**Spec:** `docs/superpowers/specs/2026-11-21-forced-update-design.md`

## Global Constraints

- The whole feature exists only with `cfg(feature = "license")`. A build without it never checks, never blocks and has no network code for updates.
- Feed: `https://github.com/castielcx/mytune-rs/releases/latest/download/version.json`, 15 s timeout, no user data.
- Signed bytes, exactly: `mytune-update-v1\n{version}\n{url}\n{sha256}`.
- URL prefix, exactly: `https://github.com/castielcx/mytune-rs/releases/download/`. The hash is 64 hex characters. The version must be strictly newer.
- The update key is separate from the licence key. The private key is at `%USERPROFILE%\.mytune\update-signing.key`, never in the repo.
- Update folder: `%ProgramData%\MyTune\updates`, DACL `D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)` (SDDL aliases for S-1-5-18 and S-1-5-32-544, never localized names).
- Setup arguments: `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH`.
- There is no continue button. The failure screen has the reason, **Retry**, **Download manually** and **Revert all tweaks**.
- The block never shows during a voice call or while a call rings. It never stops `--take-own`, `--enable-autostart`, the stream worker, saved display restore or `Engine::recover`.
- **No commit steps.** Other agents have uncommitted work in `app.rs`, `chatpage.rs`, `lib.rs`, `MyTune-rs.iss`, `installer/build.ps1` and `Cargo.lock`. Re-read a file just before editing it, make small targeted edits, and never revert or reformat other work.
- Do not update crates. New dependency edges may only point at versions already in `Cargo.lock`.
- Build and test from `mytune-rs` with `.\build.ps1 <cargo args>`. The mytune-agents mod routes test and check builds to `target\agents`. Never build `mytune-ui` by hand into `target\debug`. Never open the app on screen. Publishing is a separate step.
- UI: `th::button` (it already has `Sense::click_and_drag()` and `theme::pull`), `sb`/`rt` for text that may hold symbols, never `dsb`; no `ctx.input` inside `ctx.data_mut`; glass theme, no gradients or glow; no new window title.
- Near free in the background: the thread sleeps, and repaints only when the state changes.

## Review Focus

1. The download is cut off halfway: the screen shows a failure and the partial file is never run (test `truncated_download_is_an_error`, Task 3).
2. Setup exits, even with code 0, while MyTune is still alive: the user sees a failure, not an endless "Installing" (test `setup_exit_while_alive_is_a_failure`, Task 3).
3. A call starts or rings while the block is showing: the block turns into the banner for this session so the call can be answered (test `demote_turns_block_into_banner`, Task 2).
4. A daily check lands during an install, or a release is pulled so the feed goes back a version: the running install and the pending version are not disturbed (tests `running_install_is_not_replaced`, `older_version_is_ignored`, Task 2).
5. The owner signs with a key that does not match the one in the app: the setup build stops, instead of shipping a release no client accepts (test `sign_refuses_a_key_that_does_not_match`, Task 6).

---

### Task 1: Signed manifest check and placeholder key guard

**Files:**
- Modify: `crates/mytune-core/src/updater.rs` (rewrite)
- Modify: `crates/mytune-ui/src/settingspage.rs:15, 201-224`
- Test: `crates/mytune-core/src/updater.rs` (`mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces, always compiled: `pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");` (`sysreport.rs` uses it).
- Produces, inside `#[cfg(feature = "license")] mod enabled` with `pub use enabled::*`:
  - `pub const FEED_URL: &str`, `pub const URL_PREFIX: &str`
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub struct UpdateInfo { pub version: String, pub url: String, pub sha256: String }` (sha256 stored lower case)
  - `pub fn signed_bytes(version: &str, url: &str, sha256: &str) -> Vec<u8>`
  - `pub fn parse_version(v: &str) -> Option<Vec<u32>>` (kept as is)
  - `pub fn evaluate(feed_json: &str, current: &str, key: &VerifyingKey) -> Result<Option<UpdateInfo>, String>`
  - `pub fn check() -> Result<Option<UpdateInfo>, String>`
  - private: `parse_key(b64: &str) -> Result<VerifyingKey, String>`, `feed_url() -> String`, `current_version() -> String`

- [ ] **Step 1: Replace the old tests with the failing tests**

```rust
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const URL: &str = "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/MyTune-rs-Setup-0.3.0.exe";
// Made outside this code from the seed [7; 32]; mytune-keys tests the same pair (Task 6).
const GOLDEN_PUB: &str = "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=";
const GOLDEN_SIG: &str = "vWiBcPgW3uydpuscL6+P0xr/1BaPYDVEsbA5gFX6gr1p2974j9bdVPpUCWH5eYqzL9pQhLQnOTunc4L79/5eAA==";

fn sk() -> SigningKey { SigningKey::from_bytes(&[7; 32]) }
fn feed(k: &SigningKey, v: &str, url: &str, sha: &str) -> String {
    let sig = STANDARD.encode(k.sign(&signed_bytes(v, url, sha)).to_bytes());
    serde_json::json!({ "version": v, "url": url, "sha256": sha, "sig": sig }).to_string()
}

#[test] fn golden_vector_verifies() {
    let j = serde_json::json!({ "version": "0.3.0", "url": URL, "sha256": SHA, "sig": GOLDEN_SIG }).to_string();
    assert_eq!(evaluate(&j, "0.2.0", &parse_key(GOLDEN_PUB).unwrap()).unwrap().unwrap().version, "0.3.0");
}
#[test] fn valid_newer_manifest_is_accepted() {
    let u = evaluate(&feed(&sk(), "0.3.0", URL, SHA), "0.2.0", &sk().verifying_key()).unwrap().unwrap();
    assert_eq!((u.url.as_str(), u.sha256.as_str()), (URL, SHA));
}
#[test] fn bom_is_accepted() {
    assert!(evaluate(&format!("\u{feff}{}", feed(&sk(), "0.3.0", URL, SHA)), "0.2.0", &sk().verifying_key()).unwrap().is_some());
}
#[test] fn wrong_key_is_refused() {
    let other = SigningKey::from_bytes(&[8; 32]);
    assert!(evaluate(&feed(&other, "0.3.0", URL, SHA), "0.2.0", &sk().verifying_key()).is_err());
}
#[test] fn each_changed_field_is_refused() {
    let good = feed(&sk(), "0.3.0", URL, SHA);
    for (from, to) in [("0.3.0\"", "0.4.0\""), ("v0.3.0/", "v0.4.0/"), ("01234567", "11234567")] {
        assert!(evaluate(&good.replacen(from, to, 1), "0.2.0", &sk().verifying_key()).is_err(), "{from}");
    }
}
#[test] fn bad_url_is_refused() { // correctly signed, so only the URL rule can stop them
    for url in [
        "http://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s.exe",
        "https://github.com/castielcx/mytune-rs-evil/releases/download/v0.3.0/s.exe",
        "https://github.com/other/mytune-rs/releases/download/v0.3.0/s.exe",
        "https://github.com/castielcx/mytune-rs/releases/download/../../../../evil/s.exe",
        "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s.exe?x=1",
    ] {
        assert!(evaluate(&feed(&sk(), "0.3.0", url, SHA), "0.2.0", &sk().verifying_key()).is_err(), "{url}");
    }
}
#[test] fn bad_hash_is_refused() {
    for sha in ["abc", &"g".repeat(64), &"a".repeat(65)] {
        assert!(evaluate(&feed(&sk(), "0.3.0", URL, sha), "0.2.0", &sk().verifying_key()).is_err(), "{sha}");
    }
}
#[test] fn same_and_older_version_are_up_to_date() {
    for v in ["0.2.0", "0.1.9"] {
        assert_eq!(evaluate(&feed(&sk(), v, URL, SHA), "0.2.0", &sk().verifying_key()), Ok(None), "{v}");
    }
}
#[test] fn garbage_is_an_error() {
    for text in ["", "not json", "{}", r#"{"version":"0.3.0"}"#] {
        assert!(evaluate(text, "0.2.0", &sk().verifying_key()).is_err(), "{text}");
    }
}
#[test] fn empty_key_is_refused() { assert!(parse_key("").is_err()); }
#[test] fn embedded_key_is_valid_or_the_placeholder() {
    assert!(UPDATE_PUBLIC_KEY.is_empty() || parse_key(UPDATE_PUBLIC_KEY).is_ok());
}
```

Put them in `#[cfg(test)] mod tests` inside `enabled`, so they reach its private items and the module vanishes without `license`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `.\build.ps1 test -p mytune-core --features license updater`
Expected: FAIL to compile, `signed_bytes` and the new `evaluate` signature do not exist.

- [ ] **Step 3: Implement the module**

Keep `parse_version` and `newer`. Remove the `notes` field and every C# reference. In `evaluate`, check in this order: JSON (strip a leading BOM), all four fields present, signature with `verify_strict` over the raw strings, URL, hash, version. The URL must start with `URL_PREFIX` and contain none of `..`, `?`, `#`, `\`, `@`.

The placeholder and its guard, verbatim:

```rust
/// PLACEHOLDER: the owner's key step replaces the right-hand side with the base64 public key.
const PLACEHOLDER_NO_UPDATE_KEY: &str = "";
const UPDATE_PUBLIC_KEY: &str = PLACEHOLDER_NO_UPDATE_KEY;
const _: () = assert!(cfg!(debug_assertions) || UPDATE_PUBLIC_KEY.len() == 44, "UPDATE_PUBLIC_KEY is still the placeholder: run the key step in the forced-update plan");
```

`feed_url()` and `current_version()` return `FEED_URL` and `APP_VERSION`. Only under `#[cfg(debug_assertions)]` they first read the env vars `MYTUNE_TEST_FEED` and `MYTUNE_TEST_VERSION` (used by the final end-to-end test). A release build must contain no such read.

`check()`: parse the embedded key, fetch with `ureq::AgentBuilder::new().timeout(Duration::from_secs(15))`, then `evaluate(&text, &current_version(), &key)`.

- [ ] **Step 4: Update Settings**

In `settingspage.rs`, gate `use mytune_core::updater;` with `#[cfg(feature = "license")]`. Delete the `CheckForUpdates` flag read, the switch and `set_flag`. Put the Updates heading and row in a `#[cfg(feature = "license")]` block with only **Check now**. Row text: "MyTune looks for a new version when it starts and once a day. Nothing about you is sent." On `Ok(Some(u))` the status is `"Version {} is available. Use Update now at the top of the window."`. Leave `"CheckForUpdates"` in `settingsbackup.rs:43`: it is the C# settings format and nothing reads it.

- [ ] **Step 5: Verify tests, both feature sets and the guard**

Run: `.\build.ps1 test -p mytune-core --features license updater`
Expected: PASS, 11 tests.

Run: `.\build.ps1 check -p mytune-ui` and `.\build.ps1 check -p mytune-ui --features 'license'`
Expected: both finish without errors.

Run: `$env:CARGO_TARGET_DIR='target\agents'; .\build.ps1 check --release -p mytune-core --features license`
Expected: FAIL with `UPDATE_PUBLIC_KEY is still the placeholder`.

---

### Task 2: Update state and background check

**Files:**
- Modify: `crates/mytune-core/src/updater.rs`

**Interfaces:**
- Consumes: `UpdateInfo`, `parse_version`, `check` internals from Task 1.
- Produces:
  - `#[derive(Debug, Clone, PartialEq)] pub enum Phase { Ready, Downloading { got: u64, total: Option<u64> }, Installing, Failed(String) }`
  - `#[derive(Debug, Clone, PartialEq)] pub struct Pending { pub info: UpdateInfo, pub phase: Phase, pub blocks: bool }`
  - `pub fn pending() -> Option<Pending>`
  - `pub fn demote()` sets `blocks = false`
  - `pub fn start(update_failed: bool, repaint: impl Fn() + Send + Sync + 'static)`
  - `#[doc(hidden)] pub fn fake_pending(p: Option<Pending>)` for shot tests
  - private: `merge(now: Option<Pending>, found: UpdateInfo, blocks: bool, failed: bool) -> Option<Pending>`, `set(next: Option<Pending>)`
  - `check()` now also records a found version with `blocks = false`.

- [ ] **Step 1: Write the failing tests**

```rust
fn info(v: &str) -> UpdateInfo { UpdateInfo { version: v.into(), url: URL.into(), sha256: SHA.into() } }
fn at(v: &str, phase: Phase, blocks: bool) -> Option<Pending> { Some(Pending { info: info(v), phase, blocks }) }

#[test] fn start_check_blocks() { assert_eq!(merge(None, info("0.3.0"), true, false), at("0.3.0", Phase::Ready, true)); }
#[test] fn later_check_only_offers() { assert_eq!(merge(None, info("0.3.0"), false, false), at("0.3.0", Phase::Ready, false)); }
#[test] fn update_failed_start_shows_failure() {
    let p = merge(None, info("0.3.0"), true, true).unwrap();
    assert!(p.blocks && matches!(p.phase, Phase::Failed(_)));
}
#[test] fn running_install_is_not_replaced() {
    for phase in [Phase::Downloading { got: 1, total: None }, Phase::Installing] {
        let now = at("0.3.0", phase, true);
        assert_eq!(merge(now.clone(), info("0.4.0"), false, false), now);
    }
}
#[test] fn same_version_keeps_phase() {
    let now = at("0.3.0", Phase::Failed("x".into()), true);
    assert_eq!(merge(now.clone(), info("0.3.0"), false, false), now);
}
#[test] fn newer_version_replaces_failure_and_keeps_block() {
    assert_eq!(merge(at("0.3.0", Phase::Failed("x".into()), true), info("0.3.1"), false, false), at("0.3.1", Phase::Ready, true));
}
#[test] fn older_version_is_ignored() {
    let now = at("0.3.1", Phase::Ready, false);
    assert_eq!(merge(now.clone(), info("0.3.0"), false, false), now);
}
#[test] fn demote_turns_block_into_banner() {
    fake_pending(at("0.3.0", Phase::Ready, true));
    demote();
    assert!(!pending().unwrap().blocks);
    fake_pending(None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `.\build.ps1 test -p mytune-core --features license updater`
Expected: FAIL to compile, `merge` and `Pending` do not exist.

- [ ] **Step 3: Implement state and thread**

Keep the state in a `static Mutex<Option<Pending>>` and the repaint callback in a `OnceLock`, as `license.rs` does. `set` calls repaint only when the value changed.

`start` spawns one named thread (`update-check`):

1. Check once with `blocks = true, failed = update_failed`.
2. On `Err`, log it, sleep 60 s and check once more with the same flags.
3. Then loop forever: sleep 24 h, check with `blocks = false, failed = false`. Log errors and change nothing.

A failed check never changes the state, so an offline PC runs normally, also after `--update-failed` (the spec never blocks a PC that cannot reach GitHub, and without a manifest there is no URL to offer).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `.\build.ps1 test -p mytune-core --features license updater`
Expected: PASS, 19 tests.

---

### Task 3: Private folder, download and setup start

**Files:**
- Modify: `crates/mytune-core/src/store.rs` (after `harden_dir`, ~line 600; test after the harden test)
- Modify: `crates/mytune-core/src/updater.rs`

**Interfaces:**
- Consumes: `Pending`, `Phase`, `set`, `pending` from Task 2.
- Produces:
  - In `store.rs`: `pub(crate) const UPDATES_ACL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";` and `pub(crate) fn secure_private_dir(p: &Path) -> std::io::Result<()>`
  - In `updater.rs`:
    - `pub fn install()` starts the work on a thread. No-op while `Downloading` or `Installing`.
    - private: `updates_dir() -> PathBuf`, `clear_dir(dir: &Path) -> std::io::Result<()>`, `download(url: &str, dest: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String>`, `open_verified(path: &Path, sha256: &str) -> Result<File, String>`, `run_setup(path: &Path, held: File, args: &[&str]) -> Result<i32, String>`, `exit_text(code: i32) -> String`

- [ ] **Step 1: Write the failing tests**

In `updater.rs`:

```rust
fn serve_once(reply: &'static [u8]) -> String { /* std TcpListener on 127.0.0.1:0; one thread reads the request, writes `reply`, closes; returns the http URL */ }

#[test] fn download_saves_the_body() {
    let d = tempfile::tempdir().unwrap();
    let dest = d.path().join("s.exe");
    download(&serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"), &dest, &mut |_, _| {}).unwrap();
    assert_eq!(std::fs::read(dest).unwrap(), b"abc");
}
#[test] fn truncated_download_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let r = download(&serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"), &d.path().join("s.exe"), &mut |_, _| {});
    assert!(r.is_err());
}
#[test] fn download_does_not_write_through_an_existing_file() {
    let d = tempfile::tempdir().unwrap();
    let dest = d.path().join("s.exe");
    std::fs::write(&dest, "old").unwrap();
    assert!(download(&serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"), &dest, &mut |_, _| {}).is_err());
    assert_eq!(std::fs::read(dest).unwrap(), b"old");
}
#[test] fn open_verified_refuses_a_wrong_hash() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.exe");
    std::fs::write(&p, "abc").unwrap();
    assert!(open_verified(&p, SHA).is_err());
    assert!(open_verified(&p, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").is_ok());
}
#[test] fn verified_file_cannot_be_written_while_held() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.exe");
    std::fs::write(&p, "abc").unwrap();
    let _held = open_verified(&p, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
    assert_eq!(std::fs::OpenOptions::new().write(true).open(&p).unwrap_err().raw_os_error(), Some(32));
}
#[test] fn held_setup_still_starts() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("s.exe");
    std::fs::copy(std::env::var("ComSpec").unwrap(), &p).unwrap();
    let sha: String = Sha256::digest(std::fs::read(&p).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(run_setup(&p, open_verified(&p, &sha).unwrap(), &["/c", "exit 7"]), Ok(7));
}
#[test] fn setup_exit_while_alive_is_a_failure() {
    assert!(!exit_text(0).is_empty());
    assert!(exit_text(7).contains('7'));
}
#[test] fn clear_dir_removes_old_files() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("old.exe"), "x").unwrap();
    clear_dir(d.path()).unwrap();
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
}
```

In `store.rs` tests, skipping without elevation like the harden test:

```rust
#[test] fn secure_private_dir_closes_the_folder_and_replaces_a_link() {
    // sets up <tmp>/target and a junction <tmp>/updates -> target; skip when "O:BA" cannot be set
    // after the call: updates is a real directory, target's DACL is unchanged, and
    // read_sddl(updates, DACL) == UPDATES_ACL (so no BU entry)
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `.\build.ps1 test -p mytune-core --features license '--' updater secure_private_dir`
Expected: FAIL to compile.

- [ ] **Step 3: Implement `secure_private_dir`**

Create the folder if missing. If `symlink_metadata` says it is a link or junction, remove the link and create a real folder. Then call the existing `secure(p, UPDATES_ACL, false)`. Any error is returned. `harden_dir` can reset this folder to `DIR_ACL` on a full pass, so this runs before every download.

- [ ] **Step 4: Implement download, verification and start**

- `download`: open with `create_new(true)`. Use `AgentBuilder::timeout_connect(15 s)` and `timeout_read(30 s)`. Do **not** use `.timeout()`, which would cap the whole download. Stop with an error above 512 MiB.
- `open_verified`: open read-only with `share_mode(FILE_SHARE_READ)` (value 1), hash through that same handle, return it.
- `run_setup`: `Command::new(path).args(args).spawn()`, drop `held` only after spawn returned, wait, return the exit code. A killed process with no code returns `Err`.
- `exit_text(code)`: reason text for any exit while MyTune is alive. Code 0: "Setup ended without replacing MyTune." Other codes: "Setup stopped with code {code}. Antivirus may have blocked it."
- `install` thread, in order:
  1. Set `Downloading`.
  2. `secure_private_dir(updates_dir())`, then `clear_dir`. Stop on any error.
  3. Download to `updates\MyTune-Setup.exe`. Update `got` only when the whole percent changes.
  4. `open_verified`. On mismatch, delete the file and fail with "The download was damaged."
  5. Set `Installing`, `run_setup` with the four setup arguments.
  6. If this code runs again, setup exited and did not kill MyTune: set `Failed(exit_text(code))`.
- `clear_dir` removes the files and subfolders inside `dir`.
- First thing in the `start` thread, best effort and logged: `secure_private_dir(&updates_dir())`, and only if that succeeded, `clear_dir`. Never clear an unsecured folder: a planted junction would redirect the elevated delete. A file still locked by a finishing setup is removed at a later start.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `.\build.ps1 test -p mytune-core --features license '--' updater secure_private_dir`
Expected: PASS, 27 updater tests plus the store test.

---

### Task 4: Update screen and banner

**Files:**
- Create: `crates/mytune-ui/src/updategate.rs`
- Modify: `crates/mytune-ui/src/lib.rs:14-15` (add the module under `#[cfg(feature = "license")]`)
- Modify: `crates/mytune-ui/src/diagnosticspage.rs:471` (`fn open_url` becomes `pub(crate) fn open_url`)
- Test: `crates/mytune-ui/tests/shot_update.rs`

**Interfaces:**
- Consumes: `updater::{Pending, Phase, install, APP_VERSION}`.
- Produces:
  - `pub fn ui(root: &mut Ui, p: &Pending, can_revert: bool) -> bool`, true when **Revert all tweaks** was clicked
  - `pub fn banner(ui: &mut Ui, bar: Rect, p: &Pending)`

- [ ] **Step 1: Write the failing shot test**

`tests/shot_update.rs` starts with `#![cfg(feature = "license")]`. One test, `update_screens`, draws `updategate::ui` directly in a 1120x720 harness with `install_fonts` and `apply_visuals` (see `shot_dialog.rs`). It saves `update-ready.png`, `update-downloading.png` (`got: 42, total: Some(100)`), `update-installing.png` and `update-failed.png` to `SHOT_DIR`. It then draws `banner` in a 1120x44 rect for `Ready`, `Downloading` and `Failed`, saved as `update-banner-*.png`. It asserts each file exists and is larger than 1 KB. It uses the temp folder and no `App`, so no licence or update network call runs.

- [ ] **Step 2: Run it to verify it fails**

Run: `$env:SHOT_DIR="$env:TEMP\mytune-update-shots"; New-Item -ItemType Directory -Force $env:SHOT_DIR | Out-Null; .\build.ps1 test -p mytune-ui --features 'license' --test shot_update`
Expected: FAIL to compile, `updategate` does not exist.

- [ ] **Step 3: Implement `ui`**

Copy the structure of `licensegate::ui`: `CentralPanel`, centered `th::card`, width `440.0_f32.min(area.width() - 32.0)`. Title with `sb("Update required", 17.0)`. Line: `"MyTune {new} is ready. You have {APP_VERSION}."`.

| Phase | Content |
|---|---|
| `Ready` | **Update now** (accent), calls `updater::install()` |
| `Downloading` | "Downloading... {pct}%" and a flat progress bar; only MB when `total` is `None` |
| `Installing` | "Installing. MyTune restarts by itself." |
| `Failed(why)` | `why` in `th::DANGER`, then **Retry** (accent, `install()`), **Download manually** (`crate::diagnosticspage::open_url(&p.info.url)`), **Revert all tweaks** (`add_enabled_ui(can_revert)`) |

Use `ui.horizontal_wrapped` for the three buttons. No timer repaints: the core repaints on change.

- [ ] **Step 4: Implement `banner`**

Draw a glass pill right-aligned in `bar`, ending 8 px left of the three caption buttons (`bar.right() - 46.0 * 3.0 - 8.0`), and leave it inside the 44 px bar so no page moves.

| Phase | Pill |
|---|---|
| `Ready` | "MyTune {new} is ready" + **Update now** |
| `Downloading` | "Downloading {pct}%" |
| `Installing` | "Installing" |
| `Failed(why)` | "Update failed" + **Retry**, `why` as hover text |

The update button's hover text: "Restarts MyTune. A running call ends."

- [ ] **Step 5: Run and look at the shots**

Run the Step 2 command.
Expected: PASS. Read every PNG: text fits, the three failure buttons are all visible, and the look matches the licence card.

---

### Task 5: Gate in the app and start of the check

**Files:**
- Modify: `crates/mytune-ui/src/app.rs:470` (before the licence gate) and `:711` (end of the `title_bar` closure)
- Modify: `crates/mytune-ui/src/chatpage.rs` (next to `call_flash`, ~line 793)
- Modify: `crates/mytune-ui/src/main.rs` (after `App::new()` in the `run_native` closure)
- Test: `crates/mytune-ui/tests/shot_update.rs`

**Interfaces:**
- Consumes: `updater::{pending, demote, start, fake_pending}`, `updategate::{ui, banner}`, `App::revert_all_modal`.
- Produces: `pub fn call_busy(&self) -> bool` on `ChatPage`: true when `s.voice.active()` or any `s.rooms` entry has `incoming`; false while the chat has not started.

- [ ] **Step 1: Add the failing app-level shot**

Add test `app_shows_the_gate` to `shot_update.rs`: `App::with_store` on a temp folder, `updater::fake_pending` with `Ready, blocks: true`, run 40 frames, save `update-app.png`, clear with `fake_pending(None)`. It must not reach the licence check, because the update gate returns first. Assert the file is larger than 1 KB.

Do not run it before Step 2: without the gate, `App` reaches the licence check, which contacts the key server.

- [ ] **Step 2: Add the gate in `App::ui`, directly before the licence gate**

```rust
#[cfg(feature = "license")]
if let Some(up) = mytune_core::updater::pending().filter(|p| p.blocks) {
    if self.chat.call_busy() {
        mytune_core::updater::demote(); // a call must keep its controls; the block returns at the next start
    } else {
        th::backdrop(ctx, &ctx.layer_painter(egui::LayerId::background()), ctx.content_rect());
        self.title_bar(root, 248.0);
        let can_revert = self.states_loaded && self.work_rx.is_none() && !self.games.busy() && !self.ids_in_store.is_empty();
        if crate::updategate::ui(root, &up, can_revert) {
            self.confirm_revert_all = true;
        }
        if self.confirm_revert_all {
            self.revert_all_modal(ctx);
        }
        if let Some((msg, ok)) = self.status.take() {
            crate::notice::post(ctx, msg, ok);
        }
        crate::notice::show(ctx, 44.0);
        crate::fade::end_frame(ctx);
        return;
    }
}
```

`self.poll(ctx)` runs earlier in `ui`, so `Engine::recover`, the revert result and `games.poll` keep working behind the gate. This reuses the existing revert path (`revert_all_modal`, `run_work`, `GamesPage::revert_all`); the engine has no revert-all of its own.

- [ ] **Step 3: Add the banner to `title_bar`**

At the end of the closure, after the caption-button loop:

```rust
#[cfg(feature = "license")]
if let Some(up) = mytune_core::updater::pending().filter(|p| !p.blocks) {
    crate::updategate::banner(ui, bar, &up);
}
```

- [ ] **Step 4: Start the check in `main.rs`**

After `let mut app = mytune_ui::app::App::new();`:

```rust
#[cfg(feature = "license")]
if !PREVIEW {
    let ctx = cc.egui_ctx.clone();
    mytune_core::updater::start(std::env::args().any(|a| a == "--update-failed"), move || ctx.request_repaint());
}
```

This sits inside `run_native`, so `--take-own`, the stream worker and `--enable-autostart`, which return earlier, never check. Shot tests use `App` directly and never start the thread.

- [ ] **Step 5: Verify**

Run the Task 4 Step 2 command.
Expected: PASS, and `update-app.png` shows the title bar and update card with no sidebar or page.

Run: `.\build.ps1 check -p mytune-ui` and `.\build.ps1 check -p mytune-ui --features 'local-stickers,taskbar-look,license'`
Expected: both finish without errors.

---

### Task 6: Key and signing commands

**Files:**
- Modify: `crates/mytune-keys/Cargo.toml` (add `sha2 = "0.10"`, already locked at 0.10.9)
- Modify: `crates/mytune-keys/src/main.rs:1-3, 31-46` and `mod tests`

**Interfaces:**
- Consumes: the signed-bytes format and golden vector from Task 1.
- Produces:
  - CLI `mytune-keys update-keygen`: writes the key once, prints the public key and the path, never the private key.
  - CLI `mytune-keys update-sign <setup.exe> <version> <expected-public-base64>`: writes `version.json` next to the setup. Exit code 1 and a message on stderr on any error.
  - `fn update_key_path() -> PathBuf`
  - `fn create_update_key(path: &Path) -> Result<String, String>` (file holds the base64 of the 32-byte seed; returns the public key)
  - `fn sign_manifest(seed: &[u8; 32], version: &str, sha256: &str) -> serde_json::Value`
  - `fn sign_setup(seed: &[u8; 32], expected_public: &str, version: &str, file_name: &str, setup: &[u8]) -> Result<serde_json::Value, String>`

- [ ] **Step 1: Write the failing tests**

```rust
const SEED: [u8; 32] = [7; 32];
const PUBLIC: &str = "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=";

#[test] fn signature_matches_the_app_format() {
    let m = sign_manifest(&SEED, "0.3.0", &"0123456789abcdef".repeat(4));
    assert_eq!(m["url"], "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/MyTune-rs-Setup-0.3.0.exe");
    assert_eq!(m["sig"], "vWiBcPgW3uydpuscL6+P0xr/1BaPYDVEsbA5gFX6gr1p2974j9bdVPpUCWH5eYqzL9pQhLQnOTunc4L79/5eAA==");
}
#[test] fn sign_setup_hashes_the_file() {
    let m = sign_setup(&SEED, PUBLIC, "0.3.0", "MyTune-rs-Setup-0.3.0.exe", b"abc").unwrap();
    assert_eq!(m["sha256"], "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
#[test] fn sign_refuses_a_key_that_does_not_match() {
    assert!(sign_setup(&[8; 32], PUBLIC, "0.3.0", "MyTune-rs-Setup-0.3.0.exe", b"abc").is_err());
}
#[test] fn sign_refuses_a_setup_of_another_version() {
    assert!(sign_setup(&SEED, PUBLIC, "0.3.0", "MyTune-rs-Setup-0.2.0.exe", b"abc").is_err());
}
#[test] fn update_keygen_never_overwrites() {
    let dir = std::env::temp_dir().join(format!("mytune-keys-{}", std::process::id()));
    let path = dir.join("update-signing.key");
    let _ = std::fs::remove_dir_all(&dir);
    create_update_key(&path).unwrap();
    let first = std::fs::read(&path).unwrap();
    assert!(create_update_key(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), first);
    std::fs::remove_dir_all(&dir).unwrap();
}
```

- [ ] **Step 2: Run them to verify they fail**

First save the lock's version lines: `git diff -U0 -- Cargo.lock | Select-String '^[-+]version' | Out-File "$env:TEMP\lock-before.txt"`

Run: `.\build.ps1 test -p mytune-keys --offline '--' update sign`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Move the console-attach block from `keygen` into `fn attach_console()` and call it from all three commands. `create_update_key` makes the parent folder and writes with `create_new(true)`. `update-sign` reads the key from `update_key_path()`, and the file name comes from the setup path. Write `version.json` as pretty JSON without a BOM.

- [ ] **Step 4: Verify tests and lock**

Run: `.\build.ps1 test -p mytune-keys --offline '--' update sign`
Expected: PASS, including the 5 new tests.

Run: `git diff -U0 -- Cargo.lock | Select-String '^[-+]version' | Out-File "$env:TEMP\lock-after.txt"; Compare-Object (Get-Content "$env:TEMP\lock-before.txt") (Get-Content "$env:TEMP\lock-after.txt")`
Expected: no output (no crate version changed). The only new lock line is `"sha2 0.10.9"` under `mytune-keys`.

---

### Task 7: Installer relaunch and signing

**Files:**
- Modify: `installer/MyTune-rs.iss` (`[Run]`, `[Code]`)
- Modify: `installer/build.ps1` (after the ISCC call, before the final `Get-Item`)

**Interfaces:**
- Consumes: `mytune-keys update-sign` from Task 6, `--update-failed` from Task 5.
- Produces: `installer\output\version.json` beside the setup.

- [ ] **Step 1: Edit the Inno script**

Add to `[Run]`, keeping the existing `skipifsilent` line:

```ini
Filename: "{app}\{#AppExe}"; Flags: nowait; Check: IsRelaunch
```

In `[Code]`, add `AppStopped, InstallDone: Boolean;` to the global `var` block, set `AppStopped := True;` right after `StopApp;` in `PrepareToInstall`, set `InstallDone := True` in `CurStepChanged` when `CurStep = ssDone`, and add:

```pascal
function IsRelaunch: Boolean;
var i: Integer;
begin
  Result := False;
  for i := 1 to ParamCount do
    if CompareText(ParamStr(i), '/RELAUNCH') = 0 then Result := True;
end;

// {app} is only safe here after PrepareToInstall ran, which AppStopped proves.
procedure DeinitializeSetup;
var code: Integer;
begin
  if AppStopped and (not InstallDone) and IsRelaunch and FileExists(ExpandConstant('{app}\{#AppExe}')) then
    Exec(ExpandConstant('{app}\{#AppExe}'), '--update-failed', '', SW_SHOW, ewNoWait, code);
end;
```

- [ ] **Step 2: Compile the script without running it**

Run: `& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" /Q /DAppVersion=0.0.0 "/O$env:TEMP\mytune-iss-check" installer\MyTune-rs.iss; $LASTEXITCODE`
Expected: `0`. If ISCC complains only that a `target\release` file is missing, report the script as not compiled; do not build a release for it. Never run the output.

- [ ] **Step 3: Edit `build.ps1`**

After ISCC, in this order:

1. Read the key from `crates\mytune-core\src\updater.rs` with the regex `UPDATE_PUBLIC_KEY: &str = "([A-Za-z0-9+/=]{44})"`. Throw when it does not match.
2. With `$ErrorActionPreference = 'Continue'`, run `& "$root\build.ps1" build --release -p mytune-keys` and throw on a non-zero exit.
3. Run `& "$root\target\release\mytune-keys.exe" update-sign $setup $version $public | Write-Host`. The pipe is required: `mytune-keys` is a GUI-subsystem exe, and without it PowerShell neither waits nor sets `$LASTEXITCODE`. Throw on a non-zero exit, then restore `'Stop'`.
4. Throw if `installer\output\version.json` is missing.
5. Print, without running them:

```text
gh release create v<version> installer\output\MyTune-rs-Setup-<version>.exe installer\output\version.json --repo castielcx/mytune-rs --draft --title "MyTune <version>"
gh release edit v<version> --repo castielcx/mytune-rs --draft=false --latest
```

- [ ] **Step 4: Check the PowerShell syntax**

Run: `$e=$null; [void][System.Management.Automation.Language.Parser]::ParseFile("$PWD\installer\build.ps1",[ref]$null,[ref]$e); $e.Count`
Expected: `0`. The script cannot run end to end until the key step.

---

### Task 8: Whole-feature check and notes

**Files:**
- Modify: `docs/features.md:59` and add one entry to the `## Releases` section

**Interfaces:**
- Consumes: everything above.
- Produces: the feature history entry.

- [ ] **Step 1: Update `docs/features.md`**

Replace the stale C# line 59 (`Services\Updater.cs`) with one entry named `forced-update`: what it does, the code (`mytune-core/src/updater.rs`, `mytune-ui/src/updategate.rs`, `mytune-keys` commands, `installer`), that it exists only with `license`, the release order (draft, then published), what was measured, and what was never tried on screen. Grep first; do not read the file whole.

- [ ] **Step 2: Run the full check**

Run each:

```powershell
.\build.ps1 test -p mytune-core --features license updater
.\build.ps1 test -p mytune-core
.\build.ps1 test -p mytune-keys
.\build.ps1 check -p mytune-ui
.\build.ps1 check -p mytune-ui --features 'local-stickers,taskbar-look,license'
```

Expected: all pass. Failures in code this plan did not touch are reported, not fixed.

- [ ] **Step 3: Confirm the old feed and the flag are gone**

Run: `git grep --untracked -n -e 'castielcx/MyTune' -e 'CheckForUpdates' -- crates installer`
Expected: only `crates/mytune-core/src/settingsbackup.rs:43` (the C# settings default).

- [ ] **Step 4: Confirm a dev build cannot check**

Run: `git grep --untracked -n 'updater::' -- crates/mytune-ui/src`
Expected: every hit is inside a `#[cfg(feature = "license")]` item or in `updategate.rs`. The Task 5 check without features proves this compiles.

---

## Owner's final step (not for the implementing agent)

Until this is done, `installer\build.ps1` stops at the placeholder guard on purpose. Run the end-to-end test only in the Windows Sandbox (`installer\MyTune-Test.wsb`), because setup's `StopApp` kills any running MyTune.

1. Create the key once: `.\build.ps1 build --release -p mytune-keys`, then `target\release\mytune-keys.exe update-keygen | Write-Host`. Back up `%USERPROFILE%\.mytune\update-signing.key` offline.
2. In `updater.rs`, set `UPDATE_PUBLIC_KEY` to the printed base64 and delete `PLACEHOLDER_NO_UPDATE_KEY`.
3. Set the workspace version to `0.3.0` and run `installer\build.ps1`.
4. Confirm the test overrides are compiled out: `Select-String -Path target\release\MyTune.exe -Pattern MYTUNE_TEST_FEED -SimpleMatch -Quiet` must print `False`.
5. Upload as a draft, then publish it as a **prerelease**: `gh release edit v0.3.0 --repo castielcx/mytune-rs --draft=false --prerelease --latest=false`. Draft assets cannot be downloaded anonymously, and `/releases/latest/` never serves a prerelease, so no user receives it.
6. Build the "old" app: `.\build.ps1 build -p mytune-ui --features 'license' --target-dir target\e2e`. Copy `target\e2e\debug\MyTune.exe` into `installer\output`.
7. In the Sandbox, set `MYTUNE_TEST_VERSION=0.0.1` and `MYTUNE_TEST_FEED=https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/version.json`, and start that exe. Check:
   - the update screen blocks
   - **Update now** installs and relaunches 0.3.0
   - with networking off, the app runs normally
   - a wrong `MYTUNE_TEST_FEED` runs normally
8. Promote it: `gh release edit v0.3.0 --repo castielcx/mytune-rs --prerelease=false --latest`.
