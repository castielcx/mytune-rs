//! MyTune key manager: creates, revokes, unbinds and deletes licence keys on the key server (keyserver\worker.js).
//! `mytune-keys keygen` prints a fresh Ed25519 key pair instead of opening the window.
// No console window of its own; `keygen` attaches to the terminal it was started from to print there.
#![windows_subsystem = "windows"]

#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(process: u32) -> i32;
}

use base64::{engine::general_purpose::STANDARD, Engine as _};
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

const PKCS8_PREFIX: [u8; 16] = [0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20];
const STALE_DAYS: i64 = 30;

/// (private key as PKCS8 DER, public key as 32 raw bytes)
fn keygen() -> (Vec<u8>, [u8; 32]) {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).expect("no system random source");
    let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let mut der = PKCS8_PREFIX.to_vec();
    der.extend_from_slice(&seed);
    (der, public)
}

fn main() -> eframe::Result {
    if std::env::args().nth(1).as_deref() == Some("keygen") {
        // SAFETY: plain Win32 call; u32::MAX is ATTACH_PARENT_PROCESS. Fails harmlessly when output is piped.
        unsafe { AttachConsole(u32::MAX) };
        let (der, public) = keygen();
        println!("SIGNING_KEY (private, for `wrangler secret put SIGNING_KEY`; keep it secret):");
        println!("{}", STANDARD.encode(der));
        println!("PUBLIC_KEY (for PUBLIC_KEY in crates/mytune-core/src/license.rs):");
        println!("{}", STANDARD.encode(public));
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("MyTune keys").with_inner_size([1000.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("MyTune keys", options, Box::new(|cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Ok(Box::new(App::new(&cc.egui_ctx)))
    }))
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Config {
    url: String,
    secret: String,
}

fn config_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("MyTuneKeys").join("config.json")
}

fn load_config() -> Option<Config> {
    serde_json::from_str(&std::fs::read_to_string(config_path()).ok()?).ok()
}

fn save_config(c: &Config) -> std::io::Result<()> {
    let p = config_path();
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, serde_json::to_string_pretty(c).unwrap())
}

#[derive(Deserialize)]
struct Row {
    key: String,
    note: Option<String>,
    created: Option<i64>,
    revoked: bool,
    device: Option<String>,
    device_name: Option<String>,
    bound_at: Option<i64>,
    last_seen: Option<i64>,
}

enum Reply {
    Keys(Vec<Row>),
    Created(String),
    Done,
    Error(String),
}

/// One admin call; Ok is the response body.
fn call(cfg: &Config, method: &str, path: &str, body: Option<serde_json::Value>) -> Result<String, String> {
    let url = format!("{}{}", cfg.url.trim_end_matches('/'), path);
    let req = ureq::request(method, &url)
        .timeout(std::time::Duration::from_secs(20))
        .set("Authorization", &format!("Bearer {}", cfg.secret));
    let res = match body {
        Some(b) => req.set("Content-Type", "application/json").send_string(&b.to_string()),
        None => req.call(),
    };
    match res {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&text).ok()
                .and_then(|v| v["error"].as_str().map(str::to_owned))
                .unwrap_or(text);
            Err(format!("Server answered {code}: {msg}"))
        }
        Err(e) => Err(e.to_string()),
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// yyyy-mm-dd (UTC) from unix seconds.
fn date(t: i64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = t.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_ago(t: i64) -> String {
    match (now() - t) / 86400 {
        0 => "today".into(),
        1 => "1 day ago".into(),
        n => format!("{n} days ago"),
    }
}

fn stale(r: &Row) -> bool {
    r.device.is_some() && r.last_seen.map_or(true, |t| now() - t >= STALE_DAYS * 86400)
}

struct App {
    cfg: Config,
    server_form: Option<Config>,
    rows: Vec<Row>,
    status: String,
    busy: usize,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    new_note: Option<String>,
    created: Option<String>,
    confirm_delete: Option<String>,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let saved = load_config();
        let mut app = App {
            cfg: saved.clone().unwrap_or_default(),
            server_form: if saved.is_none() { Some(Config::default()) } else { None },
            rows: Vec::new(),
            status: String::new(),
            busy: 0,
            tx,
            rx,
            new_note: None,
            created: None,
            confirm_delete: None,
        };
        if saved.is_some() {
            app.refresh(ctx);
        }
        app
    }

    fn run(&mut self, ctx: &egui::Context, job: impl FnOnce(&Config) -> Reply + Send + 'static) {
        self.busy += 1;
        let (cfg, tx, ctx) = (self.cfg.clone(), self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(&cfg));
            ctx.request_repaint();
        });
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        self.run(ctx, |c| match call(c, "GET", "/admin/keys", None) {
            Ok(text) => match serde_json::from_str::<Vec<Row>>(&text) {
                Ok(mut rows) => {
                    rows.sort_by(|a, b| b.created.cmp(&a.created));
                    Reply::Keys(rows)
                }
                Err(e) => Reply::Error(format!("Unexpected answer: {e}")),
            },
            Err(e) => Reply::Error(e),
        });
    }

    fn action(&mut self, ctx: &egui::Context, path: &'static str, body: serde_json::Value) {
        self.run(ctx, move |c| match call(c, "POST", path, Some(body)) {
            Ok(_) => Reply::Done,
            Err(e) => Reply::Error(e),
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(reply) = self.rx.try_recv() {
            self.busy -= 1;
            match reply {
                Reply::Keys(rows) => {
                    self.rows = rows;
                    self.status.clear();
                }
                Reply::Created(key) => {
                    ctx.copy_text(key.clone());
                    self.created = Some(key);
                    self.refresh(ctx);
                }
                Reply::Done => self.refresh(ctx),
                Reply::Error(e) => self.status = e,
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let center = |title: &str| {
            egui::Window::new(title.to_owned()).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        };

        if let Some(form) = &mut self.server_form {
            let (mut save, mut cancel) = (false, false);
            center("Server").show(ctx, |ui| {
                egui::Grid::new("server").num_columns(2).show(ui, |ui| {
                    ui.label("Server URL");
                    ui.add(egui::TextEdit::singleline(&mut form.url).desired_width(360.0).hint_text("https://mytune-keys.<name>.workers.dev"));
                    ui.end_row();
                    ui.label("Admin secret");
                    ui.add(egui::TextEdit::singleline(&mut form.secret).desired_width(360.0).password(true));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    save = ui.add_enabled(!form.url.trim().is_empty() && !form.secret.is_empty(), egui::Button::new("Save")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if save && !form.url.trim().starts_with("https://") {
                // the admin secret must never go over plain http; the form stays open
                self.status = "The server address must start with https://".to_owned();
            } else if save {
                let mut c = self.server_form.take().unwrap();
                c.url = c.url.trim().to_owned();
                if let Err(e) = save_config(&c) {
                    self.status = format!("Could not save the settings: {e}");
                }
                self.cfg = c;
                self.refresh(ctx);
            } else if cancel {
                self.server_form = None;
            }
        }

        if let Some(note) = &mut self.new_note {
            let (mut create, mut cancel) = (false, false);
            center("New key").show(ctx, |ui| {
                ui.label("Note (who gets it)");
                let r = ui.add(egui::TextEdit::singleline(note).desired_width(300.0));
                r.request_focus();
                create = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    create |= ui.button("Create").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if create {
                let note = self.new_note.take().unwrap();
                self.run(ctx, move |c| match call(c, "POST", "/admin/keys", Some(serde_json::json!({ "note": note }))) {
                    Ok(text) => match serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["key"].as_str().map(str::to_owned)) {
                        Some(k) => Reply::Created(k),
                        None => Reply::Error("Unexpected answer from the server".into()),
                    },
                    Err(e) => Reply::Error(e),
                });
            } else if cancel {
                self.new_note = None;
            }
        }

        if let Some(key) = &self.created {
            let mut close = false;
            center("Key created").show(ctx, |ui| {
                ui.label(egui::RichText::new(key).monospace().size(20.0));
                ui.label("Copied to the clipboard.");
                close = ui.button("OK").clicked();
            });
            if close {
                self.created = None;
            }
        }

        if let Some(key) = self.confirm_delete.clone() {
            let (mut yes, mut no) = (false, false);
            center("Delete key").show(ctx, |ui| {
                ui.label(format!("Delete {key}? The friend using it loses MyTune once the token runs out. This cannot be undone."));
                ui.horizontal(|ui| {
                    yes = ui.button("Delete").clicked();
                    no = ui.button("Cancel").clicked();
                });
            });
            if yes {
                self.confirm_delete = None;
                self.action(ctx, "/admin/delete", serde_json::json!({ "key": key }));
            } else if no {
                self.confirm_delete = None;
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        self.poll(ctx);
        let modal = self.server_form.is_some() || self.new_note.is_some() || self.created.is_some() || self.confirm_delete.is_some();

        egui::Panel::top("bar").show(root, |ui| {
            ui.add_enabled_ui(!modal, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("New key").clicked() {
                        self.new_note = Some(String::new());
                    }
                    if ui.button("Refresh").clicked() {
                        self.refresh(ctx);
                    }
                    if ui.button("Server").clicked() {
                        self.server_form = Some(self.cfg.clone());
                    }
                    if self.busy > 0 {
                        ui.spinner();
                    }
                });
            });
            let (bound, revoked) = (self.rows.iter().filter(|r| r.device.is_some()).count(), self.rows.iter().filter(|r| r.revoked).count());
            let stale_n = self.rows.iter().filter(|r| stale(r)).count();
            ui.label(format!("{} keys, {bound} bound, {revoked} revoked. {stale_n} devices not seen for {STALE_DAYS}+ days (marked orange).", self.rows.len()));
            if !self.status.is_empty() {
                ui.colored_label(egui::Color32::LIGHT_RED, &self.status);
            }
        });

        egui::CentralPanel::default().show(root, |ui| {
            ui.add_enabled_ui(!modal, |ui| {
                egui::ScrollArea::both().show(ui, |ui| {
                    egui::Grid::new("keys").striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                        for h in ["Key", "Note", "State", "Device", "Bound", "Last seen", ""] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        let mut todo: Option<(&'static str, serde_json::Value)> = None;
                        for r in &self.rows {
                            if ui.add(egui::Label::new(egui::RichText::new(&r.key).monospace()).sense(egui::Sense::click()))
                                .on_hover_text("Click to copy").clicked()
                            {
                                ctx.copy_text(r.key.clone());
                                self.status = format!("Copied {}", r.key);
                            }
                            ui.label(r.note.as_deref().unwrap_or(""));
                            ui.label(if r.revoked { "Revoked" } else if r.device.is_some() { "Bound" } else { "Free" });
                            ui.label(match &r.device {
                                Some(d) => format!("{} ({})", r.device_name.as_deref().unwrap_or(""), &d[..d.len().min(8)]),
                                None => String::new(),
                            });
                            ui.label(r.bound_at.map(date).unwrap_or_default());
                            let seen = r.last_seen.map(days_ago).unwrap_or_else(|| "never".into());
                            if stale(r) {
                                ui.colored_label(egui::Color32::from_rgb(255, 150, 40), seen);
                            } else {
                                ui.label(seen);
                            }
                            ui.horizontal(|ui| {
                                if r.device.is_some() && ui.button("Unbind").clicked() {
                                    todo = Some(("/admin/unbind", serde_json::json!({ "key": r.key })));
                                }
                                if ui.button(if r.revoked { "Allow again" } else { "Revoke" }).clicked() {
                                    todo = Some(("/admin/revoke", serde_json::json!({ "key": r.key, "revoked": !r.revoked })));
                                }
                                if ui.button("Delete").clicked() {
                                    self.confirm_delete = Some(r.key.clone());
                                }
                            });
                            ui.end_row();
                        }
                        if let Some((path, body)) = todo {
                            self.action(ctx, path, body);
                        }
                    });
                });
            });
        });

        self.dialogs(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keygen_pkcs8_and_public_key() {
        let (der, public) = keygen();
        assert_eq!(der.len(), 48);
        assert_eq!(der[..16], PKCS8_PREFIX);
        assert_eq!(hex(&PKCS8_PREFIX), "302e020100300506032b657004220420");
        let seed: [u8; 32] = der[16..].try_into().unwrap();
        assert_eq!(ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes(), public);
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_700_000_000), "2023-11-14");
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
