//! Game Hub: the installed games (state.json `Games`, filled by `gamelib::scan`), one row each, with a dialog
//! for the game's own settings (`gameprofile`: read from the game's settings file, saved with a backup of that
//! file, put back on request). Scans and file writes run on a thread; `poll` takes the result.

use crate::theme::{self as th, dsb, rt, sb, text2, text3};
use crate::widgets::{self, caption, caption3};
use eframe::egui::{self, Align, Id, Layout, Ui};
use mytune_core::engine::{Engine, RevertOutcome, Tweak};
use mytune_core::error::Error;
use mytune_core::gamelib;
use mytune_core::gameprofile::{self, ConfigTweak, Group, Item, Kind};
use mytune_core::pergame;
use mytune_core::procs::{Procs, WinProcs};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Prof {
    None,
    /// The game has not written its settings file yet.
    NoFile,
    Ready,
}

struct Row {
    id: String,
    name: String,
    launcher: String,
    exe: String,
    installed: bool,
    running: bool,
    state: Prof,
    /// MyTune holds a copy of the settings file from before its first save.
    applied: bool,
    files: Vec<PathBuf>,
    /// Windows' "high-performance GPU" preference for this exe (works for every game).
    gpu: bool,
}

fn gpu_tweak(id: &str, name: &str, exe: &str) -> Option<pergame::GpuTweak> {
    pergame::gpu_tweak(id, name, exe).ok()
}

/// A question for the user before a change: (backup id, title).
#[derive(Clone)]
enum Ask {
    /// The game's file changed since MyTune saved it: overwrite it with the original?
    Overwrite(String, String),
    /// Delete the backup of a game that is gone?
    Forget(String, String),
}

/// What the worker threads tell the UI besides their notice.
#[derive(Default)]
struct Shared {
    ask: Option<Ask>,
    /// Backups whose revert failed: Restore offers Forget for these.
    stuck: HashSet<String>,
}

fn shared(s: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    s.lock().unwrap_or_else(|p| p.into_inner())
}

/// Forget is offered only for a settings file's backup: on a GPU or NVIDIA entry it would delete live originals.
/// Only when every failure is a missing folder or copy: after a passing write error the copy is the only original.
fn note_revert(state: &Mutex<Shared>, id: &str, failed: Option<&Error>) {
    match failed {
        None => drop(shared(state).stuck.remove(id)),
        Some(Error::Revert(fails)) if id.ends_with(":config") => {
            let mut s = shared(state);
            if fails.iter().all(|f| f.contains("use Forget")) {
                s.stuck.insert(id.to_string());
            } else {
                s.stuck.remove(id);
            }
        }
        Some(_) => {}
    }
}

/// The settings dialog of one game.
struct Dialog {
    id: String,
    items: Vec<Item>,
    /// What the controls show, as written in the file. Starts as the file's own values.
    values: Vec<Option<String>>,
    /// Which key each (target, index) named when the file was read: Save refuses if that has shifted.
    keys: Vec<(usize, usize, gameprofile::KeyAt)>,
    /// The settings come from NVIDIA App's lowest preset alone (the game has no hand-written profile).
    nvidia: bool,
}

type Notice = Option<(String, bool)>;

pub struct GamesPage {
    engine: Arc<Engine>,
    rows: Vec<Row>,
    rx: Option<mpsc::Receiver<(Notice, Vec<Row>)>>,
    /// A change (scan, apply, revert, edit of the list) is running: buttons wait for it.
    working: bool,
    scanned: bool,
    read_at: Option<Instant>,
    dialog: Option<Dialog>,
    /// Test switch (env `MYTUNE_GAME_SETTINGS`, part of a game's name): opens that game's dialog once.
    auto: Option<String>,
    shared: Arc<Mutex<Shared>>,
}

const ROW_H: f32 = 30.0;

fn snapshot(engine: &Engine) -> Vec<Row> {
    let (games, dir) = engine.with_store(|s| (s.top("Games").and_then(Value::as_array).cloned().unwrap_or_default(), s.dir().to_path_buf()));
    let procs = WinProcs.list().unwrap_or_default();
    let mut rows: Vec<Row> = games
        .iter()
        .filter_map(|g| {
            let s = |k: &str| g[k].as_str().unwrap_or("").to_string();
            let (id, name, exe) = (s("Id"), s("Name"), s("ExePath"));
            if id.is_empty() {
                return None;
            }
            let t = ConfigTweak::new(Arc::new(WinProcs), &id, &name, &exe, &dir);
            let files = t.files();
            let state = if t.profile.is_none() {
                Prof::None
            } else if files.is_empty() {
                Prof::NoFile
            } else {
                Prof::Ready
            };
            let stem = Path::new(&exe).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let gpu = gpu_tweak(&id, &name, &exe).is_some_and(|g| g.is_applied().unwrap_or(false));
            Some(Row {
                gpu,
                running: !stem.is_empty() && procs.iter().any(|p| p.name.eq_ignore_ascii_case(&stem)),
                installed: Path::new(&exe).is_file(),
                applied: engine.with_store(|s| s.get_backup(&t.id).is_some()),
                launcher: s("Launcher"),
                state,
                files,
                id,
                name,
                exe,
            })
        })
        .collect();
    rows.sort_by_key(|r| (!r.installed, r.name.to_lowercase()));
    rows
}

impl GamesPage {
    pub fn new(engine: Arc<Engine>) -> GamesPage {
        let auto = std::env::var("MYTUNE_GAME_SETTINGS").ok().map(|s| s.to_lowercase());
        GamesPage { engine, rows: vec![], rx: None, working: false, scanned: false, read_at: None, dialog: None, auto, shared: Default::default() }
    }

    /// A change is running: a new one would be dropped.
    pub fn busy(&self) -> bool {
        self.working
    }

    /// A revert of this backup failed, so Restore offers Forget.
    pub fn is_stuck(&self, id: &str) -> bool {
        shared(&self.shared).stuck.contains(id)
    }

    /// Asks before deleting the backup of a game that is gone.
    pub fn ask_forget(&mut self, id: String, title: String) {
        shared(&self.shared).ask = Some(Ask::Forget(id, title));
    }

    /// Runs `job` on a thread, then reads the list again. A list read that was under way is dropped: it
    /// would bring the state from before the job back.
    fn run(&mut self, working: bool, job: impl FnOnce(&Engine) -> Notice + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        let engine = self.engine.clone();
        std::thread::spawn(move || {
            let notice = job(&engine);
            let _ = tx.send((notice, snapshot(&engine)));
        });
        self.rx = Some(rx);
        self.working = working;
    }

    fn scan(&mut self) {
        self.scanned = true;
        self.run(true, |engine| {
            let found = gamelib::scan();
            Some(match engine.with_store(|s| gamelib::update_games(s, |g| gamelib::merge(g, &found))) {
                Ok(0) => ("No new games found".into(), true),
                Ok(1) => ("1 new game added".into(), true),
                Ok(n) => (format!("{n} new games added"), true),
                Err(e) => (format!("The game list could not be saved: {e}"), false),
            })
        });
    }

    /// Changes one game's entry in the list; `edit` returning false removes it.
    fn edit_game(&mut self, id: String, edit: impl FnOnce(&mut Value) -> bool + Send + 'static) {
        self.run(true, move |engine| {
            let r = engine.with_store(|s| {
                gamelib::update_games(s, |games| {
                    let mut edit = Some(edit);
                    games.retain_mut(|g| g["Id"] != id.as_str() || edit.take().map_or(true, |e| e(g)));
                })
            });
            r.err().map(|e| (format!("The game list could not be saved: {e}"), false))
        });
    }

    /// Reads the game's settings from its file for the dialog.
    /// shortcut: read on the UI thread (one small local file per click), move to a thread if a game's file sits on a slow drive
    fn open(&mut self, id: &str) {
        let Some(r) = self.rows.iter().find(|r| r.id == id) else { return };
        let dir = self.engine.with_store(|s| s.dir().to_path_buf());
        let t = ConfigTweak::new(Arc::new(WinProcs), &r.id, &r.name, &r.exe, &dir);
        let items = t.settings();
        let keys = items.iter().filter_map(|i| Some((i.target, i.index, t.key_at(i.target, i.index)?))).collect();
        let values = items.iter().map(|i| i.value.clone()).collect();
        self.dialog = Some(Dialog { id: r.id.clone(), items, values, keys, nvidia: gameprofile::nvidia_only(&r.name, &r.exe) });
    }

    fn save(&mut self, id: String, name: String, exe: String, values: Vec<(usize, usize, String)>, keys: Vec<(usize, usize, gameprofile::KeyAt)>) {
        let state = self.shared.clone();
        self.run(true, move |engine| {
            let dir = engine.with_store(|s| s.dir().to_path_buf());
            let t = ConfigTweak::new(Arc::new(WinProcs), &id, &name, &exe, &dir);
            if !t.keys_match(&keys, &values) {
                return Some((format!("{name}: the settings file changed while the dialog was open, so nothing was saved. The dialog now shows the file as it is."), false));
            }
            mytune_core::sysreport::restore_point_once(&*mytune_core::tweaks::system_cmd());
            let tweak_id = t.id.clone();
            Some(match engine.apply(&t.with_values(values)) {
                Ok(()) => {
                    note_revert(&state, &tweak_id, None);
                    (format!("{name}: settings saved. Put back original restores the file from before MyTune changed it."), true)
                }
                Err(e) => {
                    gameprofile::drop_stray_copies(engine, &tweak_id);
                    (format!("{name}: {e}"), false)
                }
            })
        });
    }

    fn set_gpu(&mut self, id: String, name: String, exe: String, on: bool) {
        self.run(true, move |engine| {
            let t = gpu_tweak(&id, &name, &exe)?;
            if on {
                mytune_core::sysreport::restore_point_once(&*mytune_core::tweaks::system_cmd());
                return engine.apply(&t).err().map(|e| (format!("{name}: {e}"), false));
            }
            match engine.revert(&t) {
                Ok(RevertOutcome::Reverted) => None,
                Ok(RevertOutcome::NoBackup) => Some((format!("{name}: this GPU preference was not set by MyTune, so it stays as it is."), false)),
                Err(e) => Some((format!("{name}: {e}"), false)),
            }
        });
    }

    /// Undoes a stored per-game or PC check change by its backup id (Restore, and the game's dialog). Works for a
    /// game no longer in the list. A settings file changed since MyTune saved it is not overwritten without asking.
    pub fn revert(&mut self, id: String, title: String) {
        self.start_revert(id, title, true);
    }

    fn start_revert(&mut self, id: String, title: String, ask_first: bool) {
        if self.working {
            return;
        }
        let state = self.shared.clone();
        self.run(true, move |engine| {
            let changed = engine.with_store(|s| s.get_backup(&id).cloned()).is_some_and(|b| gameprofile::changed_since_apply(&b));
            if changed && ask_first {
                shared(&state).ask = Some(Ask::Overwrite(id, title));
                return None;
            }
            let Some(t) = engine.with_store(|s| pergame::revert_only(s, &id, &title)) else {
                return Some((format!("{title}: MyTune cannot undo this here"), false));
            };
            let result = engine.revert(&*t);
            note_revert(&state, &id, result.as_ref().err());
            Some(match result {
                Ok(RevertOutcome::Reverted) if changed => (format!("{title}: original restored. The file had been changed since MyTune saved it; those changes are gone."), true),
                Ok(RevertOutcome::Reverted) => (format!("{title}: original restored"), true),
                Ok(RevertOutcome::NoBackup) => (format!("{title}: no backup, nothing changed"), false),
                Err(e) => (format!("{title}: {e}"), false),
            })
        });
    }

    /// Revert all's part for changes outside the tweak list. A game that is running is skipped and stays applied.
    pub fn revert_all(&mut self, ids: Vec<String>) {
        if ids.is_empty() || self.working {
            return;
        }
        let state = self.shared.clone();
        self.run(true, move |engine| {
            let (mut done, mut left) = (0, Vec::new());
            for id in ids {
                let Some(b) = engine.with_store(|s| s.get_backup(&id).cloned()) else { continue };
                let title = if b.title.is_empty() { id.clone() } else { b.title.clone() };
                if id.starts_with("game:") && gameprofile::backup_game_running(&b).unwrap_or(true) {
                    left.push(format!("{title}: skipped, the game is running. Close it and undo this on the Restore page."));
                    continue;
                }
                let Some(t) = engine.with_store(|s| pergame::revert_only(s, &id, &title)) else { continue };
                let result = engine.revert(&*t);
                note_revert(&state, &id, result.as_ref().err());
                match result {
                    Ok(_) => done += 1,
                    Err(e) => left.push(format!("{title}: {e}")),
                }
            }
            Some(match left.is_empty() {
                true => (format!("{done} game changes undone"), true),
                false => (format!("{done} game changes undone, {} left. {}", left.len(), left.join(" ")), false),
            })
        });
    }

    /// The questions of `Ask`; shown over whichever page asked (Games or Restore).
    pub fn modals(&mut self, ctx: &egui::Context) {
        let Some(ask) = shared(&self.shared).ask.clone() else { return };
        let (head, text, yes) = match &ask {
            Ask::Overwrite(_, t) => ("Put back the original file?", format!("{t}: the file changed after MyTune saved it, by a game update or the game's own menu. Putting back the original overwrites those changes."), "Put back"),
            Ask::Forget(_, t) => ("Forget this backup?", format!("{t}: MyTune deletes its copy of the original and this entry. This cannot be undone. Use it only for a game you uninstalled."), "Forget"),
        };
        let m = egui::Modal::new(Id::new("game_ask")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(dsb(head, 20.0));
            ui.add_space(4.0);
            ui.label(rt(text, 12.0).color(text2()));
            ui.add_space(10.0);
            let mut go = false;
            ui.horizontal(|ui| {
                go = th::button(ui, yes, true, (18.0, 8.0, 18.0, 9.0)).clicked();
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            go
        });
        if m.inner && !self.working {
            shared(&self.shared).ask = None;
            match ask {
                Ask::Overwrite(id, title) => self.start_revert(id, title, false),
                Ask::Forget(id, title) => {
                    let state = self.shared.clone();
                    self.run(true, move |engine| {
                        Some(match gameprofile::forget(engine, &id) {
                            Ok(()) => {
                                note_revert(&state, &id, None);
                                (format!("{title}: backup forgotten"), true)
                            }
                            Err(e) => (format!("{title}: {e}"), false),
                        })
                    });
                }
            }
        } else if m.should_close() {
            shared(&self.shared).ask = None;
        }
    }

    /// Takes a finished thread's result. True when a change finished (the Restore list may differ now).
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let Some((notice, rows)) = self.rx.as_ref().and_then(|r| r.try_recv().ok()) else {
            if self.rx.is_some() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            return false;
        };
        self.rows = rows;
        self.rx = None;
        if let Some(id) = self.dialog.as_ref().filter(|_| self.working).map(|d| d.id.clone()) {
            self.open(&id); // a save or a put-back finished: show the file as it is now
        }
        self.read_at = Some(Instant::now());
        if let Some((text, ok)) = notice {
            crate::notice::post(ctx, text, ok);
        }
        std::mem::take(&mut self.working)
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        // a window behind a fullscreen game still counts as shown, so focus decides too
        let seen = crate::app::window_shown() && ui.input(|i| i.viewport().focused == Some(true));
        if !self.scanned {
            self.scan(); // once a run, so the list is there without a click
        } else if seen && self.rx.is_none() && self.read_at.map_or(true, |t| t.elapsed() > Duration::from_secs(3)) {
            self.run(false, |_| None); // running state and files, only while the page is shown
        }
        if seen {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }

        let mut scan = false;
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_H);
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 180.0).max(160.0));
                    let n = self.rows.iter().filter(|r| r.installed).count();
                    ui.label(sb(if n == 1 { "1 game on this PC".to_string() } else { format!("{n} games on this PC") }, 14.0));
                    ui.add_space(4.0);
                    caption(ui, "From Steam, Epic, Ubisoft Connect, EA, Battle.net, GOG, Rockstar, Riot and the Xbox app. Tools and launchers are left out, and a game in several editions or stores is listed once.");
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text = if self.working && self.rows.is_empty() { "Scanning\u{2026}" } else { "Scan for games" };
                    scan = th::button(ui, text, true, th::BTN).clicked();
                });
            });
        });
        ui.add_space(th::GAP);

        // (row index, remove it: else open its settings)
        let mut act = None;
        let mut gpu: Option<(usize, bool)> = None;
        let mut forget = None;
        th::group(ui, |ui| {
            for (i, r) in self.rows.iter().enumerate() {
                th::card(ui, (10, 10, 14, 10), |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(ROW_H);
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 260.0).max(160.0));
                            ui.label(th::md(&r.name, 14.0).color(if r.installed { th::TEXT } else { text3() }));
                            ui.add_space(3.0);
                            let store = if r.launcher.is_empty() { "Added by hand" } else { r.launcher.as_str() };
                            if !r.installed {
                                caption3(ui, &format!("{store}  \u{00B7}  Not installed any more"));
                                return;
                            }
                            caption(
                                ui,
                                &match r.state {
                                    Prof::None => store.to_string(),
                                    Prof::NoFile => format!("{store}  \u{00B7}  Start the game once"),
                                    Prof::Ready if r.applied => format!("{store}  \u{00B7}  Settings changed by MyTune"),
                                    Prof::Ready => store.to_string(),
                                },
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !r.installed && th::button(ui, "Remove", false, th::BTN).clicked() {
                                act = Some((i, true));
                            }
                            if !r.installed && r.applied && th::button(ui, "Forget backup", false, th::BTN).on_hover_text("Deletes MyTune's copy of this game's settings file.").clicked() {
                                forget = Some(i);
                            }
                            // rows without the button keep its place, so the switches line up
                            let slot = Id::new("games_btn_w");
                            if r.installed && r.state == Prof::Ready {
                                let b = th::button(ui, "In-game settings", false, th::BTN);
                                ui.data_mut(|d| d.insert_temp(slot, b.rect.width() + ui.spacing().item_spacing.x));
                                if b.clicked() {
                                    act = Some((i, false));
                                }
                            } else if r.installed {
                                ui.add_space(ui.data(|d| d.get_temp::<f32>(slot)).unwrap_or(160.0));
                            }
                            if r.installed {
                                if th::switch(ui, r.gpu).on_hover_text("Tells Windows to run this game on the fast graphics card (laptops and PCs with two GPUs).").clicked() {
                                    gpu = Some((i, !r.gpu));
                                }
                                caption(ui, "High-performance GPU");
                            }
                            if r.running {
                                th::chip(ui, "Running", th::SUCCESS, th::success_soft());
                            }
                        });
                    });
                });
            }
        });
        if self.rows.is_empty() && !self.working {
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.label(rt("No games found on this PC.", 14.0).color(text2()));
            });
        }

        if let Some(want) = &self.auto {
            if let Some(id) = self.rows.iter().find(|r| r.state == Prof::Ready && r.name.to_lowercase().contains(want)).map(|r| r.id.clone()) {
                self.auto = None;
                self.open(&id);
            }
        }
        self.dialog_modal(ui.ctx());
        if self.working {
            return; // one change at a time; the click is not kept
        }
        if scan {
            self.scan();
        }
        if let Some((i, on)) = gpu {
            let r = &self.rows[i];
            self.set_gpu(r.id.clone(), r.name.clone(), r.exe.clone(), on);
        }
        if let Some(i) = forget {
            let r = &self.rows[i];
            self.ask_forget(pergame::game_tweak_id(&r.id, "config"), format!("{}: in-game settings", r.name));
        }
        match act {
            Some((i, true)) => self.edit_game(self.rows[i].id.clone(), |_| false),
            Some((i, false)) => self.open(&self.rows[i].id.clone()),
            None => {}
        }
    }

    fn dialog_modal(&mut self, ctx: &egui::Context) {
        let GamesPage { dialog, rows, working, .. } = self;
        let Some(d) = dialog else { return };
        let Some(r) = rows.iter().find(|r| r.id == d.id) else {
            *dialog = None;
            return;
        };
        // what Save would write: only what differs from the file
        let changed: Vec<(usize, usize, String)> = d.items.iter().zip(&d.values).filter(|(i, v)| **v != i.value).filter_map(|(i, v)| Some((i.target, i.index, v.clone()?))).collect();
        enum Do {
            Save,
            Best,
            Revert,
            Close,
        }
        let mut act = None;
        let m = egui::Modal::new(Id::new("game_settings")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(dsb(&r.name, 20.0));
            ui.add_space(4.0);
            ui.label(rt("In-game settings, read from the game's own settings file. Save copies the file first; the game may change them again later.", 12.0).color(text2()));
            if r.running {
                ui.add_space(6.0);
                ui.label(rt("The game is running and would overwrite its settings on exit. Close it to save.", 12.0).color(th::WARNING));
            }
            ui.add_space(12.0);
            if d.items.is_empty() {
                ui.label(rt("The game's settings file has none of the settings MyTune knows.", 14.0).color(text2()));
            }
            let max_h = (ctx.content_rect().height() - 300.0).max(160.0);
            egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
                for (group, title) in [(Group::Graphics, "Graphics"), (Group::Performance, "Performance")] {
                    if !d.items.iter().any(|i| i.setting.group == group) {
                        continue;
                    }
                    ui.label(sb(title, 12.0).color(text2()));
                    ui.add_space(4.0);
                    th::group(ui, |ui| {
                        for (n, item) in d.items.iter().enumerate().filter(|(_, i)| i.setting.group == group) {
                            th::card(ui, (14, 8, 14, 8), |ui| {
                                ui.horizontal(|ui| {
                                    ui.set_min_height(ROW_H);
                                    ui.label(rt(item.setting.label, 14.0).color(if item.value.is_some() { th::TEXT } else { text3() }));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if item.value.is_some() {
                                            control(ui, n, item, &mut d.values[n]);
                                        } else {
                                            ui.label(rt("Not in this game's file", 12.0).color(text3())).on_hover_text("The game has not written this setting. MyTune only changes settings that are already in the file.");
                                        }
                                    });
                                });
                            });
                        }
                    });
                    ui.add_space(12.0);
                }
            });
            for f in &r.files {
                ui.label(rt(f.to_string_lossy(), 12.0).color(text3()));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!changed.is_empty() && !r.running && !*working, |ui| {
                    if th::button(ui, "Save", true, (18.0, 8.0, 18.0, 9.0)).clicked() {
                        act = Some(Do::Save);
                    }
                });
                let (best, hint) = match d.nvidia {
                    true => ("Lowest (NVIDIA)", "Fills in the lowest preset NVIDIA App has for this game. Nothing is saved until you press Save."),
                    false => ("Competitive values", "Fills in MyTune's values for the most frames and the least blur. Nothing is saved until you press Save."),
                };
                if d.items.iter().any(|i| i.setting.best.is_some() && i.value.is_some()) && th::button(ui, best, false, (14.0, 8.0, 14.0, 9.0)).on_hover_text(hint).clicked() {
                    act = Some(Do::Best);
                }
                if r.applied && !*working && th::button(ui, "Put back original", false, (14.0, 8.0, 14.0, 9.0)).on_hover_text("Restores the settings file from before MyTune first changed it.").clicked() {
                    act = Some(Do::Revert);
                }
                if th::button(ui, "Close", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    act = Some(Do::Close);
                }
            });
        });
        if m.should_close() && act.is_none() {
            act = Some(Do::Close);
        }
        let (id, name, exe) = (r.id.clone(), r.name.clone(), r.exe.clone());
        match act {
            Some(Do::Save) => {
                let keys = d.keys.clone();
                self.save(id, name, exe, changed, keys);
            }
            Some(Do::Best) => {
                for (item, v) in d.items.iter().zip(&mut d.values) {
                    // the file's own spelling stays when it already says the same
                    match item.setting.best.filter(|_| item.value.is_some()) {
                        Some(b) if !item.value.as_deref().is_some_and(|now| now.eq_ignore_ascii_case(b)) => *v = Some(b.into()),
                        Some(_) => *v = item.value.clone(),
                        None => {}
                    }
                }
            }
            Some(Do::Revert) => self.revert(mytune_core::pergame::game_tweak_id(&id, "config"), format!("{name}: in-game settings")),
            Some(Do::Close) => self.dialog = None,
            None => {}
        }
    }
}

/// The control of one setting; `v` is its value as written in the file.
fn control(ui: &mut Ui, n: usize, item: &Item, v: &mut Option<String>) {
    match item.setting.kind {
        Kind::Switch { on, off } => {
            let is_on = v.as_deref().is_some_and(|v| !v.eq_ignore_ascii_case(off));
            if th::switch(ui, is_on).clicked() {
                *v = Some(if is_on { off } else { on }.into());
            }
        }
        Kind::Choice(list) => {
            let now = list.iter().position(|c| v.as_deref() == Some(c.1));
            let text = match (now, v.as_deref()) {
                (Some(i), _) => list[i].0.to_string(),
                (None, Some(v)) => format!("Custom ({v})"),
                (None, None) => "Game default".into(),
            };
            let rows: Vec<(String, bool)> = list.iter().enumerate().map(|(i, c)| (c.0.to_string(), Some(i) == now)).collect();
            if let Some(i) = widgets::dropdown(ui, &format!("game_setting_{n}"), 150.0, &text, &rows) {
                *v = Some(list[i].1.into());
            }
        }
        Kind::Number { min, max, decimals } => {
            let mut x = v.as_deref().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
            // a file value outside the range stays as it is until the user changes it
            if ui.add(egui::DragValue::new(&mut x).range(min..=max).clamp_existing_to_range(false).speed(1.0).max_decimals(decimals)).changed() {
                *v = Some(gameprofile::number_text(x, decimals));
            }
        }
    }
}
