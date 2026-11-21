# Soundboard in calls

## Intent
Play a Discord server's own sounds and Discord's default sounds in a voice call from MyTune's chat, with a popup that looks and behaves like Discord's, plus keyboard shortcuts that work while a game has focus.

## Goals
- Soundboard button directly next to Share Screen, in the call controls (`controls_row`) and the dock (`dock`), in `crates/mytune-ui/src/chatpage/voice.rs`. Shown only when `voice::State.soundboard` is `Some`, so also in DM calls (Discord defaults).
- Popup close to 1:1 with Discord: search on top, server icon rail on the left, collapsible sections in the engine's order (Favorites, this server, other servers with Nitro, Default), pill tiles with emoji and name.
- Tile: click plays for everyone (`Vc::Soundboard`). Hover shows preview (`Vc::PreviewSound`) and favourite star (`Vc::FavoriteSound`). Right-click can add a shortcut.
- Tiles are greyed with the reason while deafened or server-muted, because the engine ignores the play command then. Discord refusals (`SoundEvent::Refused`) are shown.
- "Shortcuts" tab in the popup: rows of sound, recorded key combo, remove.
- A shortcut press plays the sound once. Holding the key does not repeat.

## Non-goals
- Mouse-button shortcuts: a mouse hook runs on every mouse movement and can add input delay in games.
- Uploading or editing server sounds. Join sounds stay in settings.
- Any Nitro workaround: the engine's `allowed` / `playable` rules stay the only gate.

## Constraints
- No engine edits. `voice::State.soundboard` and `voice::Command` already cover list, play, preview and favourite.
- The keyboard hook (`crates/mytune-chat/src/ptt.rs`) runs only when push-to-talk needs it, a key is being recorded, or a call with soundboard access has at least one shortcut. It never blocks keys and does no work beyond a lookup in the hook procedure; playing happens off the hook thread.
- Shortcuts persist in `%ProgramData%\MyTune\chat-settings.json` through `mytune_chat::Settings`, with `#[serde(default)]` so old files load.
- New buttons use `Sense::click_and_drag()` plus `theme::pull`. Text that may hold symbols uses `sb`. Never call `ctx.input` inside `ctx.data_mut`.
- A shortcut to a sound not playable in the current call does nothing.

## Approaches
1. **UI on the engine's call state (picked).** The engine already filters sounds by permission and Nitro, so MyTune cannot offer a sound Discord would refuse.
2. Separate sound fetch through `Action::LoadSoundCatalog`: rejected, it lists sounds regardless of the current call's permissions.
3. Patching the engine: rejected, not needed and adds to the vendor edits to redo on upgrades.

The user picked: server plus Discord default sounds, Discord-like pressable tiles, keyboard shortcuts with combos.

## Left to the builder
Exact file split, popup sizing and colours within `theme.rs`, the combo storage format.
