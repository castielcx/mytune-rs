//! Game Hub: the installed games (state.json `Games`, filled by `gamelib::scan`), one row each, with the
//! settings profile of `gameprofile` (apply with a backup of the game's settings file, revert) and the
//! P-cores switch the game watcher reads. Scans, file reads and writes run on a thread; `poll` takes the result.

use crate::theme::{self as th, dsb, rt, sb, text2, text3};
use crate::widgets::{caption, caption3};
use eframe::egui::{self, Align, Id, Layout, Ui};
use mytune_core::engine::{Engine, RevertOutcome, Tweak};
use mytune_core::gamelib;
use mytune_core::gameprofile::{self, ConfigTweak};
use mytune_core::procs::{Procs, WinProcs};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Prof {
    None,
    /// The game has not written its settings file yet.
    NoFile,
    Ready,
    /// The file already has the profile's values and MyTune holds no backup.
    AlreadySet,
    Applied,
}

struct Row {
    id: String,
    name: String,
    launcher: String,
    exe: String,
    installed: bool,
    running: bool,
    p_cores: bool,
    state: Prof,
    changes: &'static [&'static str],
    files: Vec<PathBuf>,
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
    /// Id of the game whose profile waits for a yes.
    confirm: Option<String>,
    hybrid: bool,
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
            let state = if engine.with_store(|s| s.get_backup(&t.id).is_some()) {
                Prof::Applied
            } else if t.profile.is_none() {
                Prof::None
            } else if files.is_empty() {
                Prof::NoFile
            } else if t.is_applied().unwrap_or(false) {
                Prof::AlreadySet
            } else {
                Prof::Ready
            };
            let stem = Path::new(&exe).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            Some(Row {
                running: !stem.is_empty() && procs.iter().any(|p| p.name.eq_ignore_ascii_case(&stem)),
                installed: Path::new(&exe).is_file(),
                p_cores: g["PCoresOnly"] == true,
                changes: t.profile.as_ref().map_or(&[], |p| p.changes),
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
        let hybrid = mytune_core::gamewatch::CpuTopology::detect().is_hybrid;
        GamesPage { engine, rows: vec![], rx: None, working: false, scanned: false, read_at: None, confirm: None, hybrid }
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

    fn apply(&mut self, id: String, name: String, exe: String) {
        self.run(true, move |engine| {
            let dir = engine.with_store(|s| s.dir().to_path_buf());
            Some(match engine.apply(&ConfigTweak::new(Arc::new(WinProcs), &id, &name, &exe, &dir)) {
                Ok(()) => (format!("{name}: profile applied. Revert puts the original settings file back."), true),
                Err(e) => (format!("{name}: {e}"), false),
            })
        });
    }

    /// Puts a game's original settings file back; `id` is the tweak id (`game:<id>:config`). Also used by Restore.
    pub fn revert(&mut self, id: String, title: String) {
        if self.working {
            return;
        }
        self.run(true, move |engine| {
            let changed = engine.with_store(|s| s.get_backup(&id).cloned()).is_some_and(|b| gameprofile::changed_since_apply(&b));
            Some(match engine.revert(&ConfigTweak::for_revert(Arc::new(WinProcs), &id, &title)) {
                Ok(RevertOutcome::Reverted) if changed => (format!("{title}: original settings file restored. The file had been changed since the profile was applied; those changes are gone."), true),
                Ok(RevertOutcome::Reverted) => (format!("{title}: original settings file restored"), true),
                Ok(RevertOutcome::NoBackup) => (format!("{title}: no backup, nothing changed"), false),
                Err(e) => (format!("{title}: {e}"), false),
            })
        });
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
        self.read_at = Some(Instant::now());
        if let Some((text, ok)) = notice {
            crate::notice::post(ctx, text, ok);
        }
        std::mem::take(&mut self.working)
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if !self.scanned {
            self.scan(); // once a run, so the list is there without a click
        } else if self.rx.is_none() && self.read_at.map_or(true, |t| t.elapsed() > Duration::from_secs(3)) {
            self.run(false, |_| None); // running state and files, only while the page is shown
        }
        ui.ctx().request_repaint_after(Duration::from_secs(1));

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
        ui.add_space(12.0);

        // (row index, what was clicked)
        enum Act {
            Apply,
            Revert,
            Remove,
            PCores,
        }
        let mut act = None;
        th::group(ui, |ui| {
            for (i, r) in self.rows.iter().enumerate() {
                th::card(ui, (10, 10, 14, 10), |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(ROW_H);
                        ui.vertical(|ui| {
                            ui.set_max_width((ui.available_width() - 330.0).max(160.0));
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
                                    Prof::None => format!("{store}  \u{00B7}  No profile for this game yet"),
                                    Prof::NoFile => format!("{store}  \u{00B7}  Start the game once so it creates its settings file, then the profile can be applied"),
                                    Prof::Ready => format!("{store}  \u{00B7}  Profile: {}", r.changes.join(", ")),
                                    Prof::AlreadySet => format!("{store}  \u{00B7}  The game already has the profile's settings"),
                                    Prof::Applied => format!("{store}  \u{00B7}  Profile applied: {}", r.changes.join(", ")),
                                },
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !r.installed && th::button(ui, "Remove", false, th::BTN).clicked() {
                                act = Some((i, Act::Remove));
                            }
                            if r.state == Prof::Applied && th::button(ui, "Revert", false, th::BTN).clicked() {
                                act = Some((i, Act::Revert));
                            }
                            if r.state == Prof::Ready && r.installed && th::button(ui, "Apply profile", true, th::BTN).clicked() {
                                act = Some((i, Act::Apply));
                            }
                            if r.installed && self.hybrid {
                                ui.add_space(6.0);
                                if th::switch(ui, r.p_cores).clicked() {
                                    act = Some((i, Act::PCores));
                                }
                                ui.label(rt("P-cores only", 12.0).color(text2())).on_hover_text("Keeps this game on the fast cores of the CPU. Takes effect the next time the game starts while MyTune runs.");
                                ui.add_space(6.0);
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

        if self.working {
            return; // one change at a time; the click is not kept
        }
        if scan {
            self.scan();
        }
        if let Some((i, a)) = act {
            let (id, name, on) = (self.rows[i].id.clone(), self.rows[i].name.clone(), !self.rows[i].p_cores);
            match a {
                Act::Apply => self.confirm = Some(id),
                Act::Revert => self.revert(mytune_core::pergame::game_tweak_id(&id, "config"), format!("{name}: settings profile")),
                Act::Remove => self.edit_game(id, |_| false),
                Act::PCores => {
                    self.edit_game(id, move |g| {
                        g["PCoresOnly"] = Value::Bool(on);
                        true
                    });
                }
            }
        }
        if self.confirm.is_some() {
            self.confirm_modal(ui.ctx());
        }
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        let Some(i) = self.rows.iter().position(|r| Some(&r.id) == self.confirm.as_ref() && r.state == Prof::Ready) else {
            self.confirm = None;
            return;
        };
        let r = &self.rows[i];
        let m = egui::Modal::new(Id::new("game_profile")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.label(dsb(format!("Apply the profile to {}?", r.name), 20.0));
            ui.add_space(4.0);
            ui.label(rt("The settings file is copied first; Revert puts that copy back. Resolution, window mode, frame cap, textures, sensitivity, keys and sound stay as they are.", 12.0).color(text2()));
            ui.add_space(10.0);
            for c in r.changes {
                ui.label(rt(format!("\u{2022}  {c}"), 14.0));
                ui.add_space(4.0);
            }
            ui.add_space(6.0);
            for f in &r.files {
                ui.label(rt(f.to_string_lossy(), 12.0).color(text3()));
            }
            ui.add_space(10.0);
            let mut go = false;
            ui.horizontal(|ui| {
                go = th::button(ui, "Apply", true, (18.0, 8.0, 18.0, 9.0)).clicked();
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            go
        });
        let game = (r.id.clone(), r.name.clone(), r.exe.clone());
        if m.inner {
            self.apply(game.0, game.1, game.2);
            self.confirm = None;
        } else if m.should_close() {
            self.confirm = None;
        }
    }
}
