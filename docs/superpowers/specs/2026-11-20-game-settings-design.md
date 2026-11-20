# Game Hub: in-game settings dialog (2026-11-20)

## Why
The Game Hub list showed each profile's changes as a long text line and a "P-cores only" switch. The user wants the switch gone (no benefit, can hurt) and an in-game settings page per game where the important graphics and performance settings can be seen and changed from MyTune.

## Goals
- Remove "P-cores only" everywhere: UI switch, `Games[].PCoresOnly` use, affinity code in `gamewatch.rs` (CpuTopology stays if `bios.rs`/`hw.rs` use it), tests, README, examples.
- Game Hub row: name, store, short status (Running, Start the game once, No settings for this game yet, Not installed any more), button "In-game settings" (or Remove when not installed).
- Dialog over the page (in-app egui modal, like today's Apply confirm) per game:
  - GRAPHICS and PERFORMANCE groups, values read from the game's own config file each time it opens.
  - Controls: switch (on/off), choice list (e.g. Low/Medium/High/Epic), number (FPS cap) only when the file has the key.
  - "Competitive values" fills the controls with the old profile values, does not save.
  - "Save" writes through the existing Engine + ConfigTweak path: whole-file copy first, refused while the game runs, listed in Settings > Restore.
  - "Put back original" reverts (writes the copy back); shown only while applied.
- Write once: no automatic re-apply at app start or game exit. The game may change the file later; the dialog just shows the current values.

## Non-goals
Resolution, window mode, sensitivity, keybinds. Games without a profile (ROB RUN, Ghost Recon). A generic Unreal reader for unknown games.

## Approaches
1. Picked: hand-written setting list per game (key, label, kind, choices), read live, written by today's editor and engine (keeps backup/revert).
2. Direct file writes without the engine: no backup or Restore entry. Rejected.
3. Generic Unreal `sg.*` reader for any game: guesses at unchecked games. Later, maybe.

## Constraints
- Keep the editor's rules: only touched keys change, bytes and line endings kept, UTF-16 refused, `sg.*` keys may be added in UE files, others only edited.
- A second Save while already applied must keep the original first backup (revert goes to the file before MyTune ever touched it).
- GTA V and Minecraft key names are unconfirmed (files absent on this PC).

## Left to the builder
Exact per-game setting lists and value mappings, Engine reuse details for re-saving, dialog layout within the existing theme (`theme.rs` widgets, `th::group`).
