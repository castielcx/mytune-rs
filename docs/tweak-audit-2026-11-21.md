# MyTune tweak audit, 2026-11-21

This report is for the owner of MyTune. It records what the unattended audit checked, what it fixed, what still needs your decision, and what nobody proved. Paths are relative to `mytune-rs\crates` unless they start with another folder. Line numbers were read from the current code after the fixes. A finding about catalog data is located by its tweak id in `mytune-core\data\catalog.json`.

Finding ids are unique in this report. `SH` is the shared apply and revert path, `CAT1` and `CAT2` are the two catalog audits, `CODE` is the code-built tweaks, `GAME` is the game tweaks, `VER` is a finding added by the bug verifier, `REV` is an open item from the fix review, `RES` is a residual from the fix re-review, and `TEST` is the test-rule breach. Where a verifier overruled an auditor, this report uses the verifier's verdict.

## 1. Summary

The audit checked **111 tweaks**: 95 catalog tweaks (17 removed and 3 retired among them), 5 per-game tweaks, 10 game config tweaks, and 1 PC check fix for Game Mode.

| Result | Tweaks | Meaning |
|---|---:|---|
| Working | **10** | Apply and revert were established by reading the code, and the tweak has no open finding of its own. |
| Fixed | **1** | A fix landed and the tweak has no open finding left. |
| Needing attention | **98** | The tweak has at least one open finding. Many of these also received a fix. |
| Retired, no finding | **2** | `deb.location` and `deb.onedrive` are on no page, so they are not counted as working. |
| **Not live-tested** | **111** | No tweak was applied or reverted on the real system tonight. |

**"Working" means code-reviewed, not observed on the system.** It does not prove that a tweak has its intended effect or any performance benefit. The count also leaves out findings that affect every tweak alike: SH-C3, SH-D1, SH-D2, SH-D4, SH-D11, SH-D13 and SH-D14. The 10 tweaks are `general.storeupdate`, `adv.backgroundapps`, `adv.edge`, `adv.brave`, `deb.adid`, `deb.activity`, `deb.feedback`, `deb.wer`, `cust.dragoutline` and `cust.snapoff`.

**The fixes were published.** There are 16 fixes, published on 2026-11-21 at 04:31:10 with a good build. Most fixes are in shared code, so one fix can improve many tweaks that still have another open finding. Only `deb.tailored` moved from a finding to no open finding.

**Your state is unchanged.** Your `state.json` was byte-identical to the start-of-night copy after the publish, with SHA-256 `18D5FB04A5A280F4232BF6A4615ACACD5D46809705655C40E3C992E11C92DBFF`. It holds the same 20 applied tweaks. **Nothing was committed.**

## 2. Needs your attention

The first 23 entries are ranked by severity and consequence. All remaining open decisions follow, grouped by severity.

### 2.1 Breach of the night's test rules: real-system tests were run (TEST-1)

- **Severity:** High, because a forbidden GPU write happened.
- **What happened:** Fix agents ran the unfiltered `cargo test -p mytune-core --lib`, because the orchestrator's brief told them to. The test log shows 12 full runs tonight, between 02:46 and 04:14. At least 2 came from this audit, and the rest could not be attributed.
- **GPU write:** Each run wrote every NVIDIA display's current digital vibrance level back through NVAPI, including the primary display. The test is `vibrance::tests::real_gpu_set_same_level_is_accepted` (`mytune-core/src/vibrance.rs`). This is a forbidden GPU write even though the code writes the same value it just read. The values before and after were not compared, and NVAPI calls are not logged.
- **Registry writes:** Each run also wrote and deleted the HKLM scratch key and the value `MyTuneTestEntry-delete-me` in the real `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`.
- **Checked afterwards:** No leftovers remain in `HKCU\Software\MyTuneTest`, `HKLM\SOFTWARE\MyTuneTest` or the StartupApproved value. The log shows no real bcdedit, service, adapter, power-plan, task or Defender change.
- **Conditional test:** `settingsbackup::tests::import_copies_five_settings_displays_and_vpns` changes gamma only if DISPLAY9 is attached, and it is not attached.
- **Decision:** Should the 50 real-system tests be marked `#[ignore]`? The list is in `test-safety.md` in the audit scratchpad. This was not done because it is outside the audit scope. Until it is done, the unfiltered test command repeats these writes.

### 2.2 No automatic restore point although Settings promises one (SH-C3)

- **Severity:** High.
- **Risk:** A new install can apply "Optimize now" with no restore point while the UI says MyTune makes one before its first change.
- **Evidence:** `create_restore_point` (`mytune-core/src/sysreport.rs:283`) has one caller, the manual button at `mytune-ui/src/settingspage.rs:261`. The promise is at `settingspage.rs:254`. `Engine::apply` (`mytune-core/src/engine.rs:131`) never creates one. Your `state.json` holds an old `RestorePointCreated: true`, so a flag-based fix would not create one on this PC.
- **Decision:** Choose (a) create one once before a batch on the UI work thread, log a failure and continue, or (b) remove the promise from the UI and CLAUDE.md. The verifier recommends (a), outside `Engine::apply`, because the call can block for up to 120 seconds.

### 2.3 Game config revert can delete the only copy and report success (GAME-N1)

- **Severity:** High.
- **Risk:** If a game's settings folder is moved or offline, revert skips the file, deletes the only backup copy, removes the backup record, and tells you the original was restored.
- **Evidence:** The skip is at `mytune-core/src/gameprofile.rs:926`, the deletion at `:936-937`, and the success message at `mytune-ui/src/gamespage.rs:200-201`.
- **Decision:** Should MyTune keep the copy and report the problem, with an explicit "forget" action for uninstalled games? The audit recommends keeping the copy.

### 2.4 Revert all skips every game tweak (GAME-G1)

- **Severity:** High.
- **Risk:** Revert all leaves game GPU preferences and game config files changed, and its dialog count leaves them out without saying so. On your PC it would leave `game:8dfaa47d3d8e8ebd96fb904147f7cf84:gpu` applied.
- **Evidence:** `mytune-ui/src/app.rs:1187` keeps only ids found in `self.tweaks`, and game tweaks are not in that list.
- **Decision:** Should Revert all include game tweaks? Including config files would overwrite later in-game settings changes, so the dialog must say that and must handle a running game.

### 2.5 G-SYNC revert can turn G-SYNC off (CAT1-N1, CODE-N1)

- **Severity:** High.
- **Risk:** The switch is inverted. With a backup, Revert all writes 0 and turns G-SYNC off even if you had it on before MyTune. An interrupted apply can do the same through recovery. With no backup, which is the case on this PC, turning it off returns an error.
- **Evidence:** `NvidiaGsyncTweak` is at `mytune-core/src/nvidia.rs:279`. Revert all is at `mytune-ui/src/app.rs:1187`.
- **Decision:** Exclude this tweak from Revert all and recovery, or store and restore the prior state.

### 2.6 Store block may do nothing on Windows Pro (CAT1-N3)

- **Severity:** High in the audit, reduced in practice because `deb.store` is retired from every page.
- **Risk:** The description says it "Blocks the Store app entirely", but Microsoft's Policy CSP table lists `RemoveWindowsStore` as unsupported on Pro. A 24H2 report conflicts, so its behaviour on build 26200 is unproven.
- **Decision:** Test on Pro, then hide it on Pro, reword it, or delete it.

### 2.7 `adv.vbs` and `general.windowsupdate` contradict CLAUDE.md (SH-D19, CAT1-N19, CODE-N10)

- **Severity:** High as a product decision.
- **Risk:** CLAUDE.md lists "VBS off" and "Windows Update block" as left out, but both tweaks are active, on a page, and **applied on this PC**. `hypervisorlaunchtype off` also stops features that need the hypervisor, such as WSL 2 and Hyper-V, and the description does not say so.
- **Evidence:** `mytune-core/src/alltweaks.rs:134`, `mytune-core/src/layout.rs:33`, `:35` and `mytune-core/src/tweaks.rs:226`.
- **Decision:** Remove them or change the rule. If `adv.vbs` stays, add the hypervisor warning. Your applied state was not reverted, and it should not be reverted without your choice.

### 2.8 `installer/build.ps1` ships `taskbar-look` (CAT2-N22)

- **Severity:** High as a product decision.
- **Risk:** CLAUDE.md says this Explorer DLL injection is for the owner's build only, but the installer build passes the feature.
- **Evidence:** `installer/build.ps1:16` passes `local-stickers,taskbar-look,license`. The comment at `:14` says it is in the public build by the owner's choice of 2026-11-20.
- **Decision:** Confirm that choice and update CLAUDE.md, or remove the feature from the installer build.

### 2.9 Power plan sets Passive cooling (CAT1-C1, CODE-N5, CAT1-N6)

- **Severity:** Medium, confirmed by the verifier.
- **Risk:** On a PC whose firmware uses ACPI cooling control, Passive slows the CPU before raising fan speed. The tweak promises "no CPU throttling". How much this changes on your board was not established.
- **Evidence:** `mytune-core/src/tweaks.rs:431` writes `SYSCOOLPOL` AC=0. Microsoft documents 0 as Passive and 1 as Active. The base plan has AC=1, so MyTune changes it. The active MyTune plan on this PC has AC=0. The same plan also sets minimum processor state 0 at `tweaks.rs:429` and autonomous mode 0 at `:430`.
- **Decision:** The recommendation is AC=1, with DC left alone. Editing the table changes nothing until the plan is applied again. Also decide whether to keep the minimum state and autonomous mode or reword the text after a frame-time test.

### 2.10 `qol.takeown` does not elevate under UAC (CAT2-B1, CAT2-N13)

- **Severity:** High for other PCs. It works on this PC only because UAC is off (`EnableLUA=0`).
- **Risk:** With UAC on, the command runs unelevated and fails on protected files, and its window closes at once. The folder command's `/d y` answer may also be localized.
- **Evidence:** The catalog verb is `MyTuneTakeOwnership`. `HasLUAShield` draws the shield only, and elevation needs a `runas` verb.
- **Decision:** Approve a redesign and test on a UAC machine. A plain rename strands old keys and shares a verb name with "Run as administrator". A PowerShell wrapper adds a path-quoting risk.

### 2.11 Adapter revert after removal or rename (CODE-F2)

- **Severity:** High in the audit, confirmed by the verifier.
- **Risk for adapter properties:** After a NIC is removed or renamed, `Restart-NetAdapter` fails, so every revert fails and the backup stays forever. Restarts of adapters later in the list are also skipped.
- **Risk for bindings:** A renamed adapter is treated as removed, so revert succeeds, deletes the backup, and leaves the renamed adapter unbound. This is not reachable today because `Ethernet` and `Wi-Fi` match the backup.
- **Evidence:** `mytune-core/src/special.rs:155`, `:216` and `:448`.
- **Decision:** A name cannot distinguish "removed" from "renamed". The real fix stores a stable id such as `InterfaceGuid`, which adds to the backup format. Do you want that?

### 2.12 Adapter default revert can stop half way and still pass (RES-1)

- **Severity:** Medium, and reachable on this PC.
- **Risk:** `defaults()` in the adapter tweak uses a lenient read. A partial CIM failure can restore only some keywords, pass the check and delete the backup. That can leave, for example, `*InterruptModeration` at 0 while the UI shows off. No stored original is lost.
- **Evidence:** `mytune-core/src/special.rs:248-260`. It is reachable because `net.adapteradvanced` has an empty backup. The same class exists in bindings at `special.rs:395-399` (RES-2), but that path is unreachable here because your `net.bindings` backup holds three `"1"` entries.
- **Decision:** Should `defaults()` use a strict read and fail when any target adapter or keyword cannot be read?

### 2.13 `sys.norestart` likely has no effect with `NoAutoUpdate=1` (CAT2-N1)

- **Severity:** High.
- **Risk:** The card promises no restart mid-game, but the policy applies only with scheduled installs.
- **Evidence:** The ADML says it has no effect when Configure Automatic Updates is disabled, and Microsoft Learn ties it to `AUOptions=4`. This PC has `NoAutoUpdate=1` and `AUOptions=2` from `general.windowsupdate`. The effect was not tested.
- **Decision:** Set `AUOptions=4` (which conflicts with `general.windowsupdate`), use another policy, or change the text.

### 2.14 MSI mode is forced without a capability check (CODE-N8, CAT1-N24)

- **Severity:** High.
- **Risk:** `adv.msi` writes `MSISupported=1` to every PCI display and network device, creates the key if absent, and overrides a driver INF that set 0. A device that cannot use MSI could fail at the next boot. No specific failing device was established.
- **Evidence:** `mytune-core/src/special.rs:499-516`.
- **Decision:** Touch only devices whose key exists, skip value 0, and document Safe Mode recovery, or accept the risk under Experimental.

### 2.15 HKCU under a different admin account (SH-D1)

- **Severity:** High on PCs that elevate with another account.
- **Risk:** The app requires elevation and uses `HKEY_CURRENT_USER` and `LOCALAPPDATA`. If a standard user elevates with another admin account, 35 active HKCU tweaks and the Start pin file hit the admin's profile.
- **Evidence:** `mytune-core/src/reg.rs:34`, `mytune-core/src/special.rs:993` and `mytune-ui/build.rs:14`. Different-account elevation was not tested.
- **Decision:** Support this setup by resolving the shell user's SID, or warn or refuse when the token user differs from the shell user.

### 2.16 Partial apply stays applied (SH-D2)

- **Severity:** Medium.
- **Risk:** When step N fails, steps 1 to N-1 stay applied. The error is shown and the backup is kept, but the switch reads off. The change is recoverable from the Restore page.
- **Evidence:** `mytune-core/src/engine.rs:138-153` and `mytune-ui/src/app.rs:361-377`. The auto-undo runs only when apply returned Ok.
- **Decision:** Roll back automatically on an apply error, or keep the behaviour and label it "partly applied".

### 2.17 Tweaks read on with no backup (CAT2-N4, CODE-N2, CAT1-N5, CAT1-N8, CAT1-N21, CODE-N3)

- **Severity:** Medium.
- **Risk:** Turning these off writes catalog defaults instead of this PC's originals, or fails because no default exists. Some switches cannot go off at all.
- **Evidence:** 27 active tweaks read on with no backup: all 9 `sys.*`, all 8 `qol.*`, and `cust.bing`, `startrecs`, `hidesearch`, `hidetaskview`, `fileext`, `snapflyout`, `snapassist`, `verbose`, `crashdetails` and `settingshome`. Power plan, hibernation, Xbox, Spooler and tasks have no `revert_default` path, and `game.vrr` has the same problem. All 4 Xbox services are already Manual (`mytune-core/src/tweaks.rs:336`).
- **Decision:** Review the defaults and accept them, add known defaults per tweak, or show "already set outside MyTune".

### 2.18 Stale backup after a failed first apply (REV-A)

- **Severity:** Medium.
- **Risk:** For Hibernation, DirectX, VBS and CleanStart, a failed first apply keeps the captured backup. A later Undo can then restore an old value over a change you made by hand.
- **Evidence:** `mytune-core/src/engine.rs:142-147`.
- **Decision:** Should a failed first apply drop its capture when the tweak changed nothing? The old code behaved the same, so this is not a regression.

### 2.19 NVIDIA re-apply can drop a real original (REV-B)

- **Severity:** Medium.
- **Risk:** If the driver answers NOT_SUPPORTED once during a re-apply, the stored original for that setting is removed. Since the CODE-F3 fix, revert then skips that setting.
- **Evidence:** `mytune-core/src/nvidia.rs:169`.
- **Decision:** Approve the small correction: remove the key only when this run created it.

### 2.20 Old-format backups can fall through to defaults (REV-D)

- **Severity:** Medium.
- **Risk:** If every entry of an old adapter or binding backup is already at target or `"0"`, revert uses driver defaults and can change values you had before MyTune. Your current backups do not hit this.
- **Evidence:** `mytune-core/src/special.rs:338` and `:434`.
- **Decision:** Approve testing `b.data.is_empty()` instead of the filtered list.

### 2.21 Write side still uses the `*EEE` wildcard (REV-C)

- **Severity:** Medium on other drivers. No matching extra keyword exists on this PC.
- **Risk:** The read now filters exactly, but `Set-NetAdapterAdvancedProperty -RegistryKeyword` still accepts wildcards. `*EEE` can also match `AdvancedEEE`.
- **Evidence:** `mytune-core/src/special.rs:270`. The verifier found the wildcard behaviour on this PC with `*11nMode`.
- **Decision:** Approve a `Get | Where | Set` pipeline. It needs a test on a spare adapter, because each run restarts the adapter.

### 2.22 FPS caps and binds in game settings (GAME-G3, GAME-N2)

- **Severity:** Medium.
- **Risk:** The NVIDIA generator's deny list has no FPS or bind terms, so a cache update could make "Lowest (NVIDIA)" fill in a cap and remove your value. The hand-built dialogs also let you edit `FrameRateLimit` and `maxFps`, which conflicts with CLAUDE.md.
- **Evidence:** `mytune-core/src/nvcache.rs:345` and `mytune-core/src/gameprofile.rs:326`, `:341`, `:450`. The verifier found this only partly confirmed: it does not occur today, because the log shows no FPS or bind row kept for the 4 games with cache entries. The design spec lists an FPS cap control, so the two sources conflict.
- **Decision:** Deny these terms in the generator, and either remove the hand-built rows or amend the rule for explicit user edits.

### 2.23 `cust.numlock` default (CAT2-B2, CAT2-N8)

- **Severity:** Medium.
- **Risk:** Windows can rewrite this value at logoff and shutdown, which can flip the switch. Without a backup, off writes `"0"`.
- **Evidence:** The verifier overruled "certainly wrong". Microsoft documents only 0 as off and 2 as on, so `"0"` still means NumLock off. `.DEFAULT` holds `2147483648`, HKCU holds `2`, and the high bit is undocumented.
- **Decision:** Choose a value after a reboot test. The default was deliberately not changed.

### 2.24 Other open decisions: Medium

| Id | Risk and evidence | Decision |
|---|---|---|
| SH-D3 | Command and driver tweaks save originals only after applying, which conflicts with the CLAUDE.md backup rule (`tweaks.rs:484`, `special.rs:307`, `:413`, `:735`, `nvidia.rs:235`, `:337`, `pergame.rs:88`, `:156`). | Add read-only `capture`, or accept the journal-only exemption. |
| SH-D4 | Any read error counts as corruption, so a good `state.json` can be moved aside and the older `.bak` loaded (`store.rs:111-132`). | Separate I/O errors from parse errors and choose the behaviour for a temporarily unreadable file. |
| SH-D5 | Only `Start` is saved, so a delayed-auto service may come back as plain auto (`tweaks.rs:353`, `:375`). Service manager behaviour was not verified. | Save and restore `DelayedAutostart` after a VM test. |
| SH-D6 | A keyword with no stored value is now skipped and logged, so it stays at the tweak value (`special.rs:342-345`). | Choose a tested way to remove or reset such keywords. |
| SH-D7 | Explorer restart ends every session's Explorer, may relaunch it elevated, and uses bare `taskkill` and `tasklist` (`catalog.rs:114`). | Scope it to the user's session, launch unelevated, and use `system32` paths. |
| CAT1-N2 | `general.telemetry` says it stops services but writes only `Start`, so DiagTrack can run until reboot. Policy value 0 acts as 1 on Pro. | Add a restart flag or stop the services, and reword. |
| CAT1-N4, CODE-N4 | `deb.spooler` says the service starts when you print, but `sc qtriggerinfo Spooler` shows no triggers (`special.rs:636`). Printing was not tested. | Test printing, then fix the text or drop the tweak. |
| CAT1-N7 | The laptop table sets `CPMINCORES=4` despite "no core parking", and sets minimum state 100 on AC and 50 on battery despite "battery-friendly" (`tweaks.rs:443`, `:446`). | Confirm the values and text. |
| CAT1-N10 | Microsoft calls `TurnOffWindowsCopilot` legacy, and the Recall values were not confirmed. | Reword, or adopt the newer policy after testing. |
| CAT1-N11 | `OverlayTestMode=5` is unofficial, and reports on 24H2 are mixed. Nothing conclusive was found for 25H2 or 26200. | Measure on this PC, then keep or remove. |
| CAT1-N12 | Without a backup, `adv.hags` off writes `HwSchMode=2`, which turns HAGS on. The true default depends on GPU and driver. | Keep 2 or use `null`. |
| CAT1-N13 | Sources conflict on the Storage Sense default, and value `01` was not confirmed. Off without a backup may turn it on. | Check a clean profile, then fix the default and value. |
| CAT1-N20 | An older whole-value DirectX backup can remove tokens of the other two DirectX tweaks (`special.rs:585`). You are not affected now. | Convert old backups to tokens, or accept. |
| CAT2-N2 | `sys.usbsuspend` has no Microsoft source for its value. | Test on hardware, move it to the power plan setting, or remove it. |
| CAT2-N3 | Old states keep the 16 removed tweaks applied until the user opens Restore, because no Rust auto-revert exists. None are in your state. | Port the auto-revert, or accept manual Restore. |
| CAT2-N5 | `sys.shutdown` defaults write three values absent from the default hive, and one value may no longer be read. | Set defaults to null, then confirm effect and chips. |
| CAT2-N7 | `cust.hidesearch` default 1 may be wrong, because community sources give 2. | Confirm on a clean 26200 install. |
| CODE-N6 | The power plan sets `VIDEOIDLE` AC=0, so the display never turns off on AC (`tweaks.rs:434`). This is a burn-in risk on OLED, and the text does not mention it. | Keep the base timeout or disclose it. |
| CODE-N7 | Booster and the power plan tweak keep separate records of your plan, so revert can go to Balanced or delete a plan the booster is using (`tweaks.rs:496`, `:524`, `booster.rs`). | Share one record of the user's plan. |
| CODE-N9 | If the PowerShell query fails at start, `adv.msi` is not built, so its backup cannot be reverted that session (`alltweaks.rs:124`). | Build the tweak from the backup when the query fails. |
| GAME-N3 | `sg.*` keys are added when absent, which conflicts with the CLAUDE.md "existing keys only" rule (`gameprofile.rs:128`). The spec and `features.md` allow it. | Amend the rule or stop adding keys. |
| GAME-N4 | The PC check Game Mode fix writes with no backup or undo (`pccheck.rs:69`, `:101`). | Route it through the Engine, or accept. |
| GAME-N5 | With no backup, switching a game GPU tweak off does nothing and shows no message. ARC, Rocket League and WARDOGS read on with no backup. | Delete the value, or show why it stays. |
| GAME-N6 | ARC's row targets the 27 MB stub, while the 322 MB game exe is elsewhere. The preference may not reach the real process (not tested). | Re-resolve the exe, or apply to both. |
| GAME-N7 | GPU apply replaces the whole string and drops tokens such as `AppStatus=0;` (`pergame.rs:53`). | Edit token by token. |
| GAME-N8 | Defender, QoS and NVIDIA per-game tweaks have no production constructor, so an old backup cannot be reverted (`pergame.rs:88`, `:156`, `nvidia.rs:337`). None exist on this PC. | Add revert-only paths, drop support, or delete the code. |
| GAME-N10 | Revert overwrites a file changed by a game update or menu, and warns only afterwards (`gamespage.rs:200`). | Ask before overwriting, or keep it as designed. |

### 2.25 Other open decisions: Low

| Id | Risk and evidence | Decision |
|---|---|---|
| SH-D8 | Any `{current}` failure falls back to `{default}`, and the edited entry is not stored, so dual-boot apply and revert can hit different entries (`tweaks.rs:70`, `process.rs:119`). | Store the entry and fall back only on "not found". |
| SH-D9 | `is_applied` compares English `yes` and `off`, so localized bcdedit output could undo a good apply (`tweaks.rs:139`, `:203`). | Verify on German Windows, then match accordingly. |
| SH-D10 | When two tweaks share a new parent key, only the first records it, so an empty key can remain (`backup.rs:96`, `:137`). | Accept, or track key ownership. |
| SH-D11 | Odd-length or invalid UTF-16 and trailing empty `MULTI_SZ` items are normalised, so restore is not byte-exact (`reg.rs:149`). | Add a raw-bytes field if this matters. |
| SH-D12 | A non-string `DirectXUserGlobalSettings` reads as empty and is overwritten, and malformed tokens are dropped (`special.rs:557`). | Refuse unexpected types and preserve unknown tokens. |
| SH-D13 | Revert restores the first original even if you changed the value later (`backup.rs:111`). | Confirm this is intended. |
| SH-D14 | Two MyTune processes share `state.json.tmp` with no lock, so a simultaneous save can lose one side (`store.rs:277`). | Add a named mutex if needed. |
| SH-D15 | Old `net.dns` revert skips adapters that are down and then drops the backup (`special.rs:815`). It is not in your state. | Iterate the saved `ns:` keys. |
| SH-D16 | Removing the AdBlock slot can leave a numbering gap that hides later entries (`special.rs:900`). | Renumber, or leave it. |
| SH-D17 | Tasks read as applied when none exist, CleanStart checks 2 values, and absent `HibernateEnabled` counts as enabled (`special.rs:735`, `:1013`, `tweaks.rs:260`). | Tighten each check. |
| SH-D18 | An adapter backup with no originals writes driver defaults on off, or keeps erroring when none differ (`special.rs:338-340`). `net.adapteradvanced` has this empty backup. | Treat it as nothing to undo, or keep the default write. |
| CAT1-N9 | Power plan re-apply ignores failed delete and value-write results (`tweaks.rs:502-512`). This was not run. | Test re-apply, and switch plan first if needed. |
| CAT1-N14 | `general.gamebar` claims to disable Game Bar, but no value removes the app or blocks Win+G. | Reword, and check defaults on a clean profile. |
| CAT1-N15 | `general.display` defaults were not confirmed for this build. | Check on a clean profile. |
| CAT1-N16 | `adv.classicmenu` off without a backup keeps the `InprocServer32` key. | Delete the key tree on this path after a test. |
| CAT1-N17 | Two `deb.ads` policies may do nothing on Pro. | Check the CSP table and reword if needed. |
| CAT1-N18 | The `deb.chat` policy exists, but a Chat button on 26200 was not confirmed. | Check the taskbar and remove it if obsolete. |
| CAT1-N22, CODE-N12 | `adv.timer` reads only the registry while the booster reads only `GameTimer`, so the two can disagree. | Keep this, or check both. |
| CAT1-N23 | `general.nvidia` forces AF off and high-performance filtering for every game, lowering image quality. Setting ids were not checked against the driver. | Confirm they belong in a low-latency profile. |
| CAT1-N25 | Network keyword benefits were not measured, and `net.wifi` values come from one Intel driver. | Measure, or keep them labelled Experimental. |
| CAT1-N26 | A task that cannot be disabled is only logged, and the whole tweak is then undone with a generic message. | Report the failing task. |
| CAT2-N6 | Several catalog defaults are non-null while the value is absent in the default hive. | Keep explicit values or switch to null. |
| CAT2-N9 | `cust.startrecs` claims more than Microsoft documents. | Narrow the text or add values. |
| CAT2-N10 | `cust.crashdetails` says blue screen, while 24H2 and later use a black screen. No source confirms the value still works. | Test or reword. |
| CAT2-N11 | `sys.wudrivers` excludes all driver updates, but the text mentions only graphics. | Reword. |
| CAT2-N12 | `cust.bing` also removes Explorer search history suggestions. | Mention the side effect and consider the new toggle. |
| CAT2-N14 | `cust.adblock` force-install is unconfirmed on a non-domain PC for Chrome, and revert can mishandle an entry the user already had. | Check the browser policy pages and decide the user-owned case. |
| CAT2-N15, CODE-N15 | Removed tweaks keep working `apply` code, and revert can report "still on" when the original already matched. UI gating for every path was not established. | Make `apply` return the removed error and treat revert as done. |
| CAT2-N16, CODE-N13 | `net.tcp` can error forever on an incomplete backup, and `mem.compression` always enables compression (`special.rs:885`). | Confirm, or honour the backup. |
| CAT2-N17 | `qol.stickykeys` writes absolute values, so a user with Sticky Keys on loses it until revert. | Keep it, or skip when the feature is on. |
| CAT2-N18 | `cust.settingshome` replaces any existing page visibility list. | Merge tokens, or accept. |
| CAT2-N19 | `sys.longpaths` helps only apps that declare long-path awareness. | Reword. |
| CAT2-N20 | `sys.keyboard` also forces `KeyboardSpeed=31`, which the description omits. | Mention it or drop that value. |
| CAT2-N21 | The `qol.hover` taskbar preview claim is unconfirmed. | Test or reword. |
| CAT2-N23 | Sources conflict on whether `HideRecommendedSection` works on Pro. | Check on Pro by eye. |
| CODE-N11 | `deb.tasks` names Compatibility Appraiser, but two listed tasks do not exist on this PC. | Update the text. |
| CODE-N14 | `/hibernate on` may not restore a previous hiberfile type, and could fail forever without hibernation support (`tweaks.rs:276`). Neither was verified. | Test on a VM. |
| CODE-N16 | A failed adapter restart could leave it down, and the binding tweak has no network-drop flag. Neither was verified. | Test on a spare adapter. |
| CODE-N17, RES-3 | `-Name` allows wildcards. An adapter name containing `[` makes the strict read throw, so revert fails with the backup kept. | Escape names or use `InterfaceIndex`. |
| CODE-N18 | A leftover `start2.bin.bak` is never replaced, so old pins could return (`special.rs:980`). | Tie the file to the `start2` backup key. |
| CODE-N19 | Taskbar look switches read on from an in-memory flag even if the helper failed (`taskbar.rs:560`, `:603`). | Accept, or report helper state. |
| GAME-N9 | A dead `adv.timer` copy lacks production `capture` and `revert_default` (`pergame.rs:200`). | Delete it and its tests. |
| GAME-N11 | Game file writes are not atomic, and read-only state and timestamps are not stored (`gameprofile.rs:699`). | Use temp-and-replace and store attributes. |
| GAME-N12 | The running-game guard is by process name and has a check-then-write gap (`gameprofile.rs:838`). Any `javaw` blocks Minecraft edits. | Accept, or match by path. |
| GAME-N13 | Copies are deleted before the backup leaves `state.json`, so a crash in that gap leaves an Undo that fails forever (`gameprofile.rs:936`). | Delete copies after the save, or treat a missing copy as done. |

## 3. Fixed

There are 16 fixes. They were published on 2026-11-21 at 04:31:10 with a good build.

**How they were verified.** The final filtered run reported `test result: ok. 236 passed; 0 failed; 19 ignored; 0 measured; 50 filtered out; finished in 4.11s`, and `cargo check -p mytune-ui` passed. The 50 filtered tests are the real-system tests from TEST-1, so several tests for these fixes (for C4 and C5, for example) were not part of the final run. The last saved unfiltered log shows 281 passed and predates the final tests. **Real failure paths were tested with fakes only. The G2 Restore button and the UI fixes were not tried on screen.** An independent review passed every fix except C1 and C2. A re-review then failed C1 and C2 again, the guard described below was added, and no later review file covers that guard. It was confirmed by reading the current code and by its test.

| Fix | Tweak | Bug | Cause | Where | Verified by |
|---|---|---|---|---|---|
| SH-C1 | Adapter tweaks | A failed revert was reported as success and the backup was deleted. | `-ErrorAction SilentlyContinue` hid failed writes. | `mytune-core/src/special.rs:204`, `:259`, `:347` | Fake tests, code read. Read-back errors on empty output (`read_back_rows`, `:59`) and throws when no adapter is read (`:216`). |
| SH-C2 | `net.bindings` | A failure before a later success lost the backup. The verifier narrowed the original claim to this case. | Same silent error handling. | `special.rs:437-462` | Test `binding_empty_read_back_keeps_backup` (`:1352`), which passed in the final run. Same guard at `:448` and `:452`. |
| SH-C4 | CleanStart, DirectX, VBS, Hibernation, Services | Originals reached disk only after the change, so a crash could lose them. The AdBlock claim was refuted and AdBlock was not changed. | These tweaks had no `capture`. | `special.rs:1028`, `:596`, `tweaks.rs:209`, `:263`, `:349`, `engine.rs:148` | Review pass. Its tests are among the 50 filtered tests. |
| SH-C5 | `adv.vbs`, `adv.tick` | A failed BCD read was stored as "value absent". | The lenient read returned an empty string. | `tweaks.rs:101`, `:148`, `:212`, `:220` | Review pass. Its test is among the 50 filtered tests. |
| SH-C6 | All | A failed save after revert forgot the backup in memory. | The entry was removed before saving. | `store.rs:160` | Review pass, final run. |
| SH-C7 | All | A failed pre-change save left a backup in memory. | `begin` undid only the journal. | `engine.rs:119`, `:125`, `store.rs:172` | Review pass, final run. |
| CODE-F1 (read filter) | Adapter tweaks | `*EEE` could match other keywords on read. | The cmdlet treats `*` as a wildcard. | `special.rs:194` | Review pass, fake test. The write side stays open as REV-C. |
| CODE-F3 | `general.nvidia` | Revert reset settings that were never backed up. | A missing key was treated as `default`. | `nvidia.rs:178-183`, `:192` | Review pass, final run. |
| GAME-G2 | `game:<id>:gpu` | No Undo from Restore and no crash recovery. | Only `:config` ids were handled. | `mytune-core/src/gameprofile.rs:734`, `mytune-ui/src/app.rs:293`, `:1305`, `:1333`, `gamespage.rs:210` | Review pass. Not tried on screen. |
| GAME-G4 | Game config | An open dialog could write a value to the wrong setting. | Save used indexes that could shift. | `mytune-ui/src/gamespage.rs:175` | Review pass. Not tried on screen. |
| GAME-G5 | Game config | Number controls rounded fractions. | The control forced whole numbers. | `gamespage.rs:482` | Review pass. Not tried on screen. |
| VER-N1 (DragValue clamp) | Game config | A file value outside the range silently became a pending edit. | `DragValue` clamps existing values by default. | `gamespage.rs:482` | Review pass. Not tried on screen. |
| GAME-G6 | Game config | A UTF-8 BOM hid the first INI section. | The BOM was kept when parsing the first line. | `gameprofile.rs:146`, `:151`, `:267` | Review pass, final run. |
| T1 (CAT1-C2) | `deb.tailored` | The details omitted a value the tweak writes. | Missing text. | `mytune-core/data/catalog.json`, entry at line 1072 | Review pass. |
| T2 (CAT1-C3) | `general.nvidia` | The details omitted AA transparency supersampling and anisotropic filtering. | Missing text. | `catalog.json:79` | Review pass. |
| CODE-F4 | `net.bindings` | Text said "connected" although every enabled adapter is covered, including `NO_ADAPTER`. | Wrong wording. | `catalog.json:808`, `special.rs:110` | Review pass. |

## 4. Full table

**Legend.** Apply and revert say `traced` when the path was read in code. No row was executed on the real system. `verified` means the current state was read on this PC. It does not prove a round trip or a benefit. `documented` means Microsoft or vendor documentation supports the setting. `unverified` means neither could be established, and it does not mean the tweak is broken. Where two audits gave different labels, the table uses the lower one. No row says `sandbox`, because no audit relied on the sandbox test.

| # | Id | Source | Apply | Revert | Effect | How verified | Open findings |
|---:|---|---|---|---|---|---|---|
| 1 | `general.gamebar` | catalog | traced | traced | unverified | code | CAT1-N14 |
| 2 | `general.powerplan` | catalog | traced | traced, no path without backup | verified | code | CAT1-C1, CAT1-N6, CAT1-N7, CAT1-N8, CAT1-N9, CODE-N2, CODE-N5, CODE-N6, CODE-N7 |
| 3 | `general.nvidia` | catalog | traced | traced, no default path | unverified | code | CAT1-N23, REV-B |
| 4 | `general.telemetry` | catalog | traced | traced | documented | code, documentation | CAT1-N2 |
| 5 | `general.display` | catalog | traced | traced | unverified | code | CAT1-N15 |
| 6 | `general.windowsupdate` | catalog | traced | traced | unverified | code | SH-D19, SH-D10 |
| 7 | `general.storeupdate` | catalog | traced | traced | documented | code, documentation | none |
| 8 | `adv.timer` | catalog | traced | traced | unverified | code | CAT1-N22, CODE-N12 |
| 9 | `adv.backgroundapps` | catalog | traced | traced | unverified | code | none |
| 10 | `adv.copilot` | catalog | traced | traced | documented | code, documentation | CAT1-N10 |
| 11 | `adv.storagesense` | catalog | traced | traced | unverified | code | CAT1-N13 |
| 12 | `adv.notifications` | catalog | traced | traced | unverified | code | SH-D10 |
| 13 | `adv.hibernation` | catalog | traced | traced | unverified | code | SH-D17, CODE-N2, CODE-N14 |
| 14 | `adv.classicmenu` | catalog | traced | traced, key stays without backup | verified | code | CAT1-N16 |
| 15 | `adv.edge` | catalog | traced | traced | unverified | code | none |
| 16 | `adv.brave` | catalog | traced | traced | unverified | code | none |
| 17 | `adv.windowed` | catalog | traced | traced | unverified | code | CAT1-N20, SH-D12 |
| 18 | `adv.mpo` | catalog | traced | traced | unverified | code | CAT1-N11 |
| 19 | `adv.hags` | catalog | traced | traced | verified | code | CAT1-N12 |
| 20 | `adv.msi` | catalog | traced | traced, no default path | unverified | code | CAT1-N24, CODE-N8, CODE-N9 |
| 21 | `adv.vbs` | catalog | traced | traced | unverified | code | SH-D19, SH-D8, SH-D9, CAT1-N19, CODE-N10 |
| 22 | `net.adapterpower` | catalog | traced | traced | unverified | code | CODE-F2, RES-1, REV-C, REV-D, SH-D6, SH-D18, CAT1-N25, CODE-N16, CODE-N17 |
| 23 | `net.adapteradvanced` | catalog | traced | traced | unverified | code | CODE-F2, RES-1, REV-C, REV-D, SH-D6, SH-D18, CAT1-N25, CODE-N16, CODE-N17 |
| 24 | `net.wifi` | catalog | traced | traced | unverified | code | CODE-F2, RES-1, REV-C, REV-D, SH-D6, SH-D18, CAT1-N25, CODE-N16, CODE-N17 |
| 25 | `net.buffers` | catalog, removed | not on UI | traced, revert only | unverified | code | CODE-N15, REV-C |
| 26 | `net.bindings` | catalog | traced | traced | unverified | code | CODE-F2, RES-2, RES-3, REV-D, CODE-N16, CODE-N17 |
| 27 | `deb.adid` | catalog | traced | traced | unverified | code | none |
| 28 | `deb.ads` | catalog | traced | traced | unverified | code | CAT1-N17 |
| 29 | `deb.activity` | catalog | traced | traced | unverified | code | none |
| 30 | `deb.tailored` | catalog | traced | traced | documented | code, documentation | none (T1 fixed) |
| 31 | `deb.feedback` | catalog | traced | traced | unverified | code | none |
| 32 | `deb.location` | catalog, retired | traced, on no page | traced | unverified | code | none |
| 33 | `deb.wer` | catalog | traced | traced | unverified | code | none |
| 34 | `deb.tasks` | catalog | traced | traced | unverified | code | SH-D17, CAT1-N26, CODE-N2, CODE-N11 |
| 35 | `deb.xbox` | catalog | traced | traced | verified | code | CAT1-N5, CODE-N2, CODE-N3 |
| 36 | `deb.spooler` | catalog | traced | traced | verified | code | CAT1-N4, CAT1-N5, CODE-N2, CODE-N4 |
| 37 | `deb.onedrive` | catalog, retired | traced, on no page | traced | unverified | code | none |
| 38 | `deb.chat` | catalog | traced | traced | unverified | code | CAT1-N18 |
| 39 | `deb.store` | catalog, retired | traced, on no page | traced | documented | code, documentation | CAT1-N3 |
| 40 | `general.gsync` | catalog | traced | traced, forces off | unverified | code | CAT1-N1, CODE-N1 |
| 41 | `game.vrr` | catalog | traced | traced, no path without backup | unverified | code | CAT1-N20, CAT1-N21 |
| 42 | `game.autohdr` | catalog | traced | traced | unverified | code | CAT1-N20 |
| 43 | `sys.deliveryopt` | catalog | traced | traced | documented | code, documentation | CAT2-N4 |
| 44 | `sys.wudrivers` | catalog | traced | traced | documented | code, documentation | CAT2-N4, CAT2-N11 |
| 45 | `sys.norestart` | catalog | traced | traced | documented | code, documentation | CAT2-N1, CAT2-N4, SH-D10 |
| 46 | `sys.shutdown` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N5 |
| 47 | `sys.menudelay` | catalog | traced | traced | unverified | code | CAT2-N4 |
| 48 | `sys.startupdelay` | catalog | traced | traced | unverified | code | CAT2-N4 |
| 49 | `sys.usbsuspend` | catalog | traced | traced | unverified | code | CAT2-N2, CAT2-N4 |
| 50 | `sys.keyboard` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N20 |
| 51 | `sys.longpaths` | catalog | traced | traced | documented | code, documentation | CAT2-N4, CAT2-N19 |
| 52 | `qol.thispc` | catalog | traced | traced | unverified | code | CAT2-N4 |
| 53 | `qol.navpane` | catalog | traced | traced | unverified | code | CAT2-N4 |
| 54 | `qol.seconds` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N6 |
| 55 | `qol.searchhighlights` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N6 |
| 56 | `qol.stickykeys` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N17 |
| 57 | `qol.hover` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N21 |
| 58 | `qol.cleanmenu` | catalog | traced | traced | unverified | code | CAT2-N4 |
| 59 | `qol.takeown` | catalog | traced, fails under UAC | traced | documented | code, documentation | CAT2-B1, CAT2-N4, CAT2-N13 |
| 60 | `cust.dark` | catalog | traced | traced | unverified | code | CAT2-N6 |
| 61 | `cust.adblock` | catalog | traced | traced | unverified | code | SH-D16, CAT2-N14 |
| 62 | `cust.clearstart` | catalog | traced | traced | unverified | code | CAT2-N22, CODE-N19 |
| 63 | `cust.outlineselect` | catalog | traced | traced | unverified | code | CAT2-N22, CODE-N19 |
| 64 | `cust.dragoutline` | catalog | traced | traced | unverified | code | none |
| 65 | `cust.bing` | catalog | traced | traced | documented | code, documentation | CAT2-N4, CAT2-N12, SH-D10 |
| 66 | `cust.startrecs` | catalog | traced | traced | documented | code, documentation | CAT2-N4, CAT2-N6, CAT2-N9 |
| 67 | `cust.cleanstart` | catalog | traced | traced | documented | code, documentation | SH-D10, SH-D17, CAT2-N6, CAT2-N23, CODE-N18 |
| 68 | `cust.taskbarleft` | catalog | traced | traced | documented | code, documentation | CAT2-N6 |
| 69 | `cust.hidesearch` | catalog | traced | traced, default doubtful | unverified | code | CAT2-N4, CAT2-N7 |
| 70 | `cust.hidetaskview` | catalog | traced | traced | documented | code, documentation | CAT2-N4, CAT2-N6 |
| 71 | `cust.fileext` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N6 |
| 72 | `cust.hiddenfiles` | catalog | traced | traced | unverified | code | CAT2-N6 |
| 73 | `cust.snapoff` | catalog | traced | traced | unverified | code | none |
| 74 | `cust.snapflyout` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N6 |
| 75 | `cust.snapassist` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N6 |
| 76 | `cust.numlock` | catalog | traced | traced, default open | unverified | code | CAT2-B2, CAT2-N8 |
| 77 | `cust.verbose` | catalog | traced | traced | documented | code, documentation | CAT2-N4 |
| 78 | `cust.crashdetails` | catalog | traced | traced | unverified | code | CAT2-N4, CAT2-N10 |
| 79 | `cust.settingshome` | catalog | traced | traced | documented | code, documentation | CAT2-N4, CAT2-N18 |
| 80 | `adv.fso` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 81 | `adv.mmcss` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 82 | `adv.priosep` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 83 | `adv.powerthrottling` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 84 | `adv.irqspread` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 85 | `net.nagle` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 86 | `net.throttling` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 87 | `mem.pagefile` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 88 | `mem.standby` | catalog, removed | refused from UI | traced, old backup | unverified | code | CAT2-N3 |
| 89 | `adv.search` | catalog, removed | hidden, code remains | traced from backup | unverified | code | SH-D5, CAT2-N3, CAT2-N15, CODE-N15 |
| 90 | `adv.sysmain` | catalog, removed | hidden, code remains | traced from backup | unverified | code | SH-D5, CAT2-N3, CAT2-N15, CODE-N15 |
| 91 | `adv.services` | catalog, removed | hidden, code remains | traced from backup | unverified | code | SH-D5, CAT2-N3, CAT2-N15, CODE-N15 |
| 92 | `adv.tick` | catalog, removed | hidden, code remains | traced from backup | unverified | code | SH-D8, SH-D9, CAT2-N3, CAT2-N15, CODE-N15 |
| 93 | `net.dns` | catalog, removed | returns error | traced | unverified | code | SH-D15, CAT2-N3 |
| 94 | `net.tcp` | catalog, removed | returns error | traced | unverified | code | CAT2-N3, CAT2-N16 |
| 95 | `mem.compression` | catalog, removed | returns error | traced, always enables | unverified | code | CAT2-N3, CAT2-N16, CODE-N13 |
| 96 | `game:<id>:gpu` | per-game generator | traced | traced | unverified | code | GAME-G1, GAME-N5, GAME-N6, GAME-N7 |
| 97 | `game:<id>:defender` | per-game generator | not reachable | traced | unverified | code | GAME-N8 |
| 98 | `game:<id>:qos` | per-game generator | not reachable | traced | unverified | code | GAME-N8 |
| 99 | `game:<id>:nvidia` | per-game generator | not reachable | traced | unverified | code | GAME-N8 |
| 100 | `adv.timer#pergame-duplicate` | per-game, dead code | not reachable | traced | unverified | code | GAME-N9 |
| 101 | `game:<id>:config#arc-raiders` | game config | traced | traced | unverified | code | GAME-G1, GAME-N1, GAME-N2, GAME-N3, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 102 | `game:<id>:config#wardogs` | game config | traced | traced | unverified | code | GAME-G1, GAME-N1, GAME-N2, GAME-N3, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 103 | `game:<id>:config#rocket-league` | game config | traced | traced | unverified | code | GAME-G1, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 104 | `game:<id>:config#gta-v-enhanced` | game config | traced | traced | unverified | code | GAME-G1, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 105 | `game:<id>:config#gta-v-legacy` | game config | traced, file absent | traced | unverified | code | GAME-G1, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 106 | `game:<id>:config#ghost-recon-breakpoint` | game config | traced | traced | unverified | code | GAME-G1, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 107 | `game:<id>:config#ghost-recon-wildlands` | game config | traced, file absent | traced | unverified | code | GAME-G1, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 108 | `game:<id>:config#minecraft` | game config | traced, files absent | traced | unverified | code | GAME-G1, GAME-N1, GAME-N2, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 109 | `game:<id>:config#rob-run` | game config | traced | traced | unverified | code | GAME-G1, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 110 | `game:<id>:config#nvidia-cache` | game config generator | traced | traced | unverified | code | GAME-G1, GAME-G3, GAME-N1, GAME-N10, GAME-N11, GAME-N12, GAME-N13 |
| 111 | `pccheck:Fix::GameMode` | PC check | traced | none | unverified | code | GAME-N4 |

The inventory also lists two shared classes that are not tweaks and so have no row. `class:DataTweak` is the shared behaviour of plain registry tweaks, and `class:ConfigTweak` is the shared behaviour of the game config tweaks. Their findings are in the rows above and in the SH and GAME entries.

## 5. Not verified

- **No tweak was applied or reverted on the real system.** Every apply, revert, crash, lock and power-loss result in this report comes from reading code or from tests with fakes.
- **No benefit was measured.** No tweak was shown to improve FPS, frame times, latency, or anything else.
- **German Windows output was not tested.** This PC is en-US, so `bcdedit`, `powercfg`, adapter status strings and `takeown` answers remain unverified on German Windows.
- **Different-account elevation was not tested.**
- **Real driver and CIM failures were not reproduced.** Failure paths were exercised with fakes and stand-in cmdlets.
- **Non-Intel network drivers were not tested.** NVIDIA driver effects and MSI on specific devices were also not established.
- **In-game effects were not tested** for any config key or GPU preference.
- **GTA V Legacy, Ghost Recon Wildlands and Minecraft keys are unconfirmed**, because their files are absent on this PC.
- **C# backup key formats could not be checked**, because the C# source no longer exists.
- **Catalog defaults are mostly unconfirmed.** Current values on this PC show your state, not clean Windows defaults.
- **The UI fixes and the G2 Restore button were not tried on screen.**
- **Vibrance values before and after the test runs were not compared**, as described in TEST-1.
- **The publish time and the good-build result come from the orchestrator's record.** This report did not re-run the publish.
