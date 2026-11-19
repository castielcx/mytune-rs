# mytune-rs

Rust port of MyTune 2: a Windows gaming-tuning app with a glass-style egui UI and a built-in Discord chat
(the chat engine is [Yapfast](https://github.com/yapfast/yapfast)). Licensed GPL-3.0-or-later (see `LICENSE` and
`NOTICES.txt`).

## Read this first
- **Experimental.** It has only been run on one Windows 11 PC. Nothing here is certified to work on yours.
- **Windows only, needs administrator rights.** It changes registry values, services, power plans and NVIDIA driver
  profiles. Every tweak saves the original values to `%ProgramData%\MyTune\state.json` first, a restore point is
  created before the first change, and a tweak can be reverted from the app. Use it at your own risk.
- **Download:** a Windows installer (`MyTune-rs-Setup-0.1.0.exe`) is on the
  [Releases page](https://github.com/castielcx/mytune-rs/releases). It is not code-signed, so Windows SmartScreen will
  warn; check the SHA-256 in the release notes. You can also build it yourself (below). The Discord part has not been
  tested against live calls or screen sharing.

## Building
1. Install Rust (stable, `x86_64-pc-windows-msvc`), Visual Studio Build Tools (C++ workload), LLVM (clang-cl), NASM,
   CMake, Ninja and Git (all on PATH; `winget install` works for each).
2. Get the Yapfast engine, version 0.3.0, next to this file and apply the MyTune changes:
   ```
   git clone --branch v0.3.0 https://github.com/yapfast/yapfast vendor-src/yapfast
   git -C vendor-src/yapfast apply ../../patches/yapfast-0.3.0-mytune.patch
   ```
3. `.cargo/config.toml` points `LK_CUSTOM_WEBRTC` at a local copy of libwebrtc (`vendor-src/webrtc`). Delete that
   line to let the build download it instead.
4. Get epaint 0.37.0 (egui's drawing library) into `vendor-src/epaint` and apply the MyTune change to it (the
   smooth corner curve). The source is the `epaint-0.37.0` folder cargo keeps under
   `%USERPROFILE%\.cargo\registry\src\`:
   ```
   git -C vendor-src/epaint init
   git -C vendor-src/epaint apply ../../patches/epaint-0.37.0-mytune.patch
   ```
5. `.\build.ps1 build --release -p mytune-ui`, then run `target\release\MyTune.exe` from an elevated terminal.

## The look, and the copy for comparing (2026-11-19)
- One system in `crates/mytune-ui/src/theme.rs`: Inter as the typeface (in the exe, `assets/fonts`), one scale of
  corner radii (`R_PANEL` 18, `R_CARD` 14, `R_FLOAT` 16, `R_MENU` 12, `R_CTL` 9), the same smooth corner on every
  shape (the epaint change above), a one-pixel rim lit from above on every plate, and springs for everything that
  moves (`spring`, `anim`). Rows that belong together share one plate (`group`).
- `src/lens.rs` is real glass: an OpenGL pass that copies the frame so far and draws it back bent at a shape's
  rim, magnified, or blurred. Used by the switch knob and slider thumb while held, the tab thumb, and floating
  panels (`theme::float`, frosted). The headless screenshot tests draw with wgpu and skip it; see it with
  `.\build.ps1 run -p mytune-ui --example lens_demo --features local-stickers,preview --target-dir target`
  (opens a window, saves a picture to `LENS_OUT`, closes).
- Feature `preview` of `mytune-ui` builds a copy that runs beside the installed MyTune: window title
  "MyTune Redesign", its own single-instance name, no Win+Y, no speaker tray icon, no call rejoin. Never with
  `taskbar-look`. Run it under another file name (`MyTuneRedesign.exe`): scripts that stop `MyTune` then leave it
  alone. Both copies write one `state.json`; `Store::save` merges with what the other wrote, but a MyTune built
  before that change still overwrites the file whole.

## Toolchain
Rustup stable (`x86_64-pc-windows-msvc`) + Visual Studio Build Tools (C++ workload). In a new shell:
`cd mytune-rs` then `cargo test`.

## mytune-core (`crates/mytune-core/src`)
| File | What |
|---|---|
| `reg.rs` | HKLM/HKCU read/write/delete, all value types, key create/delete, 64-bit view |
| `backup.rs` | `RegBackup` / `TweakBackup`, same JSON as C# (DWord = unsigned decimal, QWord = **signed** decimal, Binary = base64) |
| `store.rs` | `%ProgramData%\MyTune\state.json`: atomic save (`ReplaceFileW`), `.bak`, `.corrupt-<ts>`; only `Applied` is typed, all other keys round-trip as raw JSON |
| `log.rs` | `mytune.log`, C# format (BOM, CRLF, repeat collapsing, 1 MB rotation) |
| `process.rs` | run powercfg/bcdedit/sc/schtasks, capture exit code + output, timeout kills the tree |
| `engine.rs` | apply/revert with backup; `RegistryTweak` |
| `tweaks.rs` | services-to-manual, power plan, hibernation, VBS, dynamic tick |
| `catalog.rs` + `data/catalog.json` | the 57 active + 9 removed plain registry tweaks, loaded from a **dump of the real C# catalog** (123 values). Regenerate the dump when the C# catalog changes |
| `special.rs` | DirectX prefs (3), scheduled tasks (COM), adapter properties + protocol bindings + MSI mode (PowerShell), catalog service instances, revert-only DNS / TCP stack / memory compression |

| `nvapi.rs` | NVAPI driver-profile (DRS) access: `nvapi64.dll` loaded at run time, ids/structs from NVIDIA's public header |
| `nvidia.rs` | `general.nvidia` (Base Profile), `general.gsync`, per-game profile (`NvidiaGameTweak`); `Target::Named` aims the same code at a throwaway profile for tests |

| `pergame.rs` | per-game GPU preference, Defender exclusion (SecurityRisk), QoS policy (ids `game:<id>:gpu\|defender\|qos`), and `TimerResolutionTweak` (registry + `Settings.GameTimer` flag in `state.json` via the engine's `on_applied` / `on_reverted` hooks) |

| `booster.rs` | game booster: 0.5 ms timer, quiet toasts, performance power plan, background throttle while a game runs; state saved before each change so a crash is undone on the next start |
| `throttle.rs` | background throttle (browsers, launchers, updaters; never Steam / Discord / Riot): below-normal priority + efficiency mode, restored afterwards |
| `procs.rs` | process access behind a trait (real: Toolhelp, `SetPriorityClass`, EcoQoS, affinity) |
| `gamewatch.rs` | `CpuTopology` (P-cores / V-Cache), `GameWatcher` (running games from `state.json`, P-core affinity, play time), `GameService` loop |

Everything in the C# catalog and the per-game builders is now ported, plus the game booster. Run it with
`cargo run --release --example booster_service` (refuses to start while the C# MyTune runs; Ctrl+C puts everything back).
`session.rs`: `SessionTracker`, session history in `state.json` `Sessions` (newest first, max 50, under 1 min dropped), run from `GameService::tick`. Sensor samples are wired by `mytune_sensors::session_feed::attach(&sensors, &mut service)` (also sets the sensor cadence from the running games); heat warnings come from `heatwatch.rs` (logged, no balloon; `Settings.HeatWarning`, default on); with none pushed, temps/loads are null. FPS fields stay null / `[]` (no frame capture). No balloon: `describe()` gives the text, it is logged. Same as C#: a game switch or launcher + game is one session, averages are per sample, a game already running at start begins at detection time.
Not ported: the GPU overclock hook of the booster, sensors / FPS capture (another chat), and the UI.
Known limitation kept from C#: with several games using QoS the "Do not use NLA" value ends as `1`, not absent.
Explorer restart / settings broadcast after a tweak go through the `Hooks` trait (`SystemHooks` for real).

## Revert rules (do not weaken)
- Originals are captured before any write; only the first capture of a value counts.
- Backup is saved even if apply fails half way.
- Revert without a backup changes nothing.
- A failed revert returns an error and keeps the backup; it is removed only after full success.

## Differences from C# (all stricter)
- Backup with no registry entries: revert restores nothing (C# writes the tweak's default values).
- bcdedit `/deletevalue` of a value that was absent: a non-zero exit is fine only if re-reading shows it gone.
- Dynamic tick restore is checked; services `is_applied` ignores "a backup exists".
- Corrupt `state.json` is moved (not copied) to `.corrupt-<ts>` so the next save cannot overwrite the good `.bak`.
- Process output that is not UTF-8 is decoded as the OEM code page.

## Tests
- `cargo test`: 60 headless unit tests (HKCU `Software\MyTuneTest`, temp dirs, fake command runner / task API)
  plus `catalog_sandbox`: applies and reverts all 66 catalog registry tweaks with HKLM/HKCU/HKU of the test
  process redirected (`RegOverridePredefKey`) to scratch keys, with and without original values.
- `cargo test --test real_system -- --ignored --test-threads=1` (elevated): real `sc`/`bcdedit` on a dummy
  service and a throwaway copy of the boot entry, both deleted afterwards.
  Also a dummy scheduled task through COM, and read-only checks that the PowerShell output parses.
- `cargo test --test real_nvidia -- --ignored --nocapture`: real NVAPI on throwaway driver profiles only
  (`MyTuneTest-<pid>`, `MyTune - MyTuneTestGame`); the Base Profile is read before and after and must be identical.
  The "unsupported setting" probe is per profile (Ansel is refused on the Base Profile, accepted on custom ones).
- `cargo test --test real_pergame -- --ignored --nocapture --test-threads=1` (elevated): a real QoS policy
  (created and removed, NLA value aimed at a scratch key) and a READ-ONLY Defender check. Defender apply/revert
  is never run for real (it would change a security setting); the fake-runner tests cover it.
- `cargo test --test real_booster -- --ignored --nocapture --test-threads=1` (elevated): real CPU topology, timer,
  priority throttling of a dummy process, and an end-to-end service run with dummy "game" / "background" processes
  (copies of ping.exe), quiet mode on a scratch key and a switch to a THROWAWAY power plan and back (guard restores the plan).
- Not covered by real tests: power plan, hibernation, adapter property / binding changes, DNS / TCP /
  memory-compression reverts (fake runner only; changing them for real would touch the live network or plan).
- Integration tests set `MYTUNE_DIR` so they never write the real log or state.
- Examples: `write_sample_state` (feeds the C# reader check), `smoke_readonly` (read-only output-format check).
