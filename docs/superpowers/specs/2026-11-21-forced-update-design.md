# Forced one-click update: design

Date: 2026-11-21. Status: waiting for the owner's approval.

## Why

Every installed copy of MyTune should end up on the newest release. When a newer version is on GitHub, the app tells the user, and from the next app start it does not work until they update. Updating takes one click.

The existing check cannot do this. It reads the deleted C# app's feed (`castielcx/MyTune`), compares against a hardcoded `2.4.3`, runs only from a button in Settings, and cannot download or install.

## Goals

- Check at app start and every 24 hours on every public install.
- Read releases from `castielcx/mytune-rs`. Remove every reference to the C# feed.
- From the next app start, a newer version replaces the whole UI with an update screen.
- One click on **Update now** downloads, verifies, installs silently and restarts MyTune.
- Only releases signed with the owner's private key can be installed.

## Non-goals

- No install without a click.
- No block during a voice call, because the block hides the call controls. The update screen never closes a game or a call.
- No block on a machine that cannot reach GitHub.
- No delta updates, update channels, rollback UI, telemetry or new server.
- The owner's dev build (`target\debug\MyTune.exe`) is never checked or blocked.

## Anti-goals

- A tampered or invalid feed must not lock users out or run anything.
- The update screen must not trap a user with tweaks they cannot revert.
- The check must stay near free while the window is hidden or a game runs.

## Approaches

1. **Fully automatic.** Installs with no click when idle or at next start. Rejected by the owner.
2. **One-click (chosen).** Same check and download path, and the user presses Update. The user sees when it happens.
3. **Link to a manual download.** Least code, but users must find and run the setup themselves. Rejected because it makes a forced update painful.

## Behaviour

### Check

- Runs only in builds with the `license` feature. The setup build has it (`installer\build.ps1`). The publish build of the daily app does not.
- A background thread fetches `https://github.com/castielcx/mytune-rs/releases/latest/download/version.json` with the existing `ureq` client and a 15 s timeout. It sends no user data.
- At start: one try, then one retry after 60 s because autostart can run before the network is up.
- Then every 24 hours while the app runs. The thread sleeps between checks and requests a repaint only when the result changes.

### What the result does

| Result | Effect |
|---|---|
| Same or older version | Nothing |
| Network error, 404, or a release with no `version.json` | Nothing, app runs normally, logged |
| Bad signature, bad URL, bad hash format | Ignored, app runs normally, logged |
| Valid newer version, found by the start check or its 60 s retry, no voice call active | Update screen replaces the UI. After a `--minimized` start it appears when the window is first opened |
| Same, while a voice call is active | Non-blocking banner with **Update now**. The block comes at the next app start |
| Valid newer version, found by a later 24-hour check | Non-blocking banner with **Update now**. The block comes at the next app start |

### Update screen

- Drawn in `app.rs` before the licence gate, the same way the licence gate blocks: no sidebar, no pages. An update may be what fixes a licence problem, so it comes first.
- Follows the existing licence gate layout and the glass theme.
- Shows the current and new version and an **Update now** button.
- During the update it shows download progress, then "Installing".
- On failure it shows the reason, **Retry**, **Download manually** (opens the setup URL from the signed manifest in the browser) and **Revert all tweaks** (the existing revert path).
- There is no way past the screen. The app stays locked until the update succeeds (owner's decision, 2026-11-21).
- The block lives only in the UI. `--take-own`, `--enable-autostart`, the stream worker, saved display restore and `Engine::recover` keep running, as they do behind the licence gate.

### Install

1. Download the setup to `%ProgramFiles%\MyTune Updates\`, resolved with `SHGetKnownFolderPath`. That folder is restricted to SYSTEM and Administrators, because an admin process runs what is inside it. It is not under `%ProgramData%`, where a standard user can pre-create and own the parent folder. The uninstaller removes it.
2. Check the SHA-256 against the signed manifest. Keep the file open without write sharing until setup has started, so it cannot be swapped after the check.
3. Start it with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH`. MyTune already runs as admin, so there is no UAC prompt.
4. MyTune stays open and waits on the setup process. If setup cannot start, or exits with an error before closing MyTune (for example because antivirus blocked it), MyTune shows the failure on the update screen.
5. Setup's existing `StopApp` closes MyTune, installs, and restarts it because of `/RELAUNCH`. If the install fails after that point, setup restarts MyTune from `[Code]` with `--update-failed` so it shows the failure. If setup itself is killed, the user reopens MyTune and sees the update screen again.
6. Delete old files in `updates\` at the next start.

`installer\MyTune-rs.iss` needs a new `[Run]` entry that starts the app only when `/RELAUNCH` was passed. The existing `skipifsilent` entry stays for normal installs.

## Signing and the feed

`version.json` is a release asset:

```json
{
  "version": "0.3.0",
  "url": "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/MyTune-rs-Setup-0.3.0.exe",
  "sha256": "<64 hex>",
  "sig": "<base64 Ed25519 signature>"
}
```

- The signature covers the exact bytes `mytune-update-v1\n{version}\n{url}\n{sha256}`.
- The app accepts a manifest only if the signature verifies against a public key built into the app, the URL starts with `https://github.com/castielcx/mytune-rs/releases/download/`, the hash is 64 hex characters, and the version is strictly newer. The strict check blocks downgrades to an old signed release.
- The update key is new and separate from the licence key, so a leak of one does not expose the other.
- The private key lives in `%USERPROFILE%\.mytune\update-signing.key`, never in the repo or on GitHub.
- `mytune-keys` gets two commands: one creates the key pair once and prints the public key, one signs a built setup and writes `version.json`.
- `installer\build.ps1` calls the signing command after ISCC and puts `version.json` next to the setup. Both are uploaded to a draft GitHub release, which is published only afterwards, so no client sees a manifest whose setup is missing.

## Code changes

- `crates/mytune-core/src/updater.rs`: new feed URL, version from `CARGO_PKG_VERSION`, signature check, download, install start, tests.
- `crates/mytune-ui/src/app.rs`: check thread, gate and banner.
- New update screen next to `licensegate.rs`.
- `crates/mytune-ui/src/settingspage.rs`: remove the **Check for updates** toggle, since the check is now mandatory, and fix its text. Keep **Check now**.
- `crates/mytune-keys`: key creation and signing commands.
- `installer/MyTune-rs.iss` and `installer/build.ps1`.
- `docs/features.md`: new entry, and remove the stale C# updater notes.

## Known limits

- Copies on 0.2.0 have no updater. Each must install the first release with this feature by hand.
- A PC where MyTune never restarts sees only the banner, never the block.
- A broken release cannot be recalled. The fix is to publish a higher version, and users on the broken one need its updater to still work.
- MyTune is not code-signed, so antivirus may block the download or setup. That shows as a failure on the update screen, and the user stays locked out until the manual download works or a fixed release ships.
- If the private key is lost, no new update can be accepted, and every user must install a new build by hand. The owner must back it up offline.
- If the private key leaks, an attacker who can also publish to the GitHub repo can run code as admin on every machine.

## Left to the builder

- Module layout, thread and state plumbing.
- Exact wording and layout of the screen and banner, within the page and glass rules.
- Version comparison code.
- How the folder ACL is set.

## Verification

- Unit tests: valid manifest, wrong key, changed field, bad URL, bad hash, same version, older version.
- Headless screenshots of the update screen and the banner.
- End-to-end on a test install with a test key and a local or draft feed: older version blocks, one click updates and relaunches, offline runs normally.
- A dev build never checks.
