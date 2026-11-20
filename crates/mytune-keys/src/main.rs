//! MyTune key manager: creates, revokes, unbinds and deletes licence keys on the key server (keyserver\worker.js).
//! `mytune-keys keygen` prints a fresh Ed25519 key pair instead of opening the window.
// No console window of its own; `keygen` attaches to the terminal it was started from to print there.
#![windows_subsystem = "windows"]

#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(process: u32) -> i32;
    fn GetStdHandle(which: u32) -> isize;
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
        // SAFETY: plain Win32 calls. Attach (u32::MAX = parent process) only when stdout (-11) is not
        // already a pipe or file: attaching would replace it and a script would read nothing.
        unsafe {
            if matches!(GetStdHandle(-11i32 as u32), 0 | -1) {
                AttachConsole(u32::MAX);
            }
        }
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

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Key,
    Note,
    State,
    Device,
    Seen,
    Created,
}

const ORANGE: egui::Color32 = egui::Color32::from_rgb(255, 150, 40);
const RED: egui::Color32 = egui::Color32::from_rgb(240, 90, 90);
const GREEN: egui::Color32 = egui::Color32::from_rgb(90, 200, 120);

fn state_of(r: &Row) -> (&'static str, egui::Color32) {
    if r.revoked {
        ("Revoked", RED)
    } else if r.device.is_some() {
        ("In use", GREEN)
    } else {
        ("Free", egui::Color32::from_gray(150))
    }
}

/// A small coloured badge.
fn badge(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(egui::RichText::new(text).color(color)));
}

struct App {
    cfg: Config,
    server_form: Option<Config>,
    rows: Vec<Row>,
    /// (text, is an error)
    status: Option<(String, bool)>,
    busy: usize,
    tx: Sender<Reply>,
    rx: Receiver<Reply>,
    new_note: Option<String>,
    created: Option<String>,
    confirm_delete: Option<String>,
    search: String,
    sort: Col,
    ascending: bool,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let saved = load_config();
        let mut app = App {
            cfg: saved.clone().unwrap_or_default(),
            server_form: if saved.is_none() { Some(Config::default()) } else { None },
            rows: Vec::new(),
            status: None,
            busy: 0,
            tx,
            rx,
            new_note: None,
            created: None,
            confirm_delete: None,
            search: String::new(),
            sort: Col::Created,
            ascending: false,
        };
        if saved.is_some() {
            app.refresh(ctx);
        }
        app
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), false));
    }

    fn error(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), true));
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
                Ok(rows) => Reply::Keys(rows),
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
                    if self.status.as_ref().is_some_and(|s| s.1) {
                        self.status = None;
                    }
                }
                Reply::Created(key) => {
                    ctx.copy_text(key.clone());
                    self.created = Some(key);
                    self.refresh(ctx);
                }
                Reply::Done => self.refresh(ctx),
                Reply::Error(e) => self.error(e),
            }
        }
    }

    /// Rows matching the search, in the chosen order.
    fn visible(&self) -> Vec<&Row> {
        let q = self.search.trim().to_lowercase();
        let mut rows: Vec<&Row> = self.rows.iter()
            .filter(|r| q.is_empty() || [Some(&r.key), r.note.as_ref(), r.device_name.as_ref()]
                .into_iter().flatten().any(|s| s.to_lowercase().contains(&q)))
            .collect();
        let lower = |s: &Option<String>| s.as_deref().unwrap_or("").to_lowercase();
        rows.sort_by(|a, b| {
            let o = match self.sort {
                Col::Key => a.key.cmp(&b.key),
                Col::Note => lower(&a.note).cmp(&lower(&b.note)),
                Col::State => state_of(a).0.cmp(state_of(b).0),
                Col::Device => lower(&a.device_name).cmp(&lower(&b.device_name)),
                Col::Seen => a.last_seen.cmp(&b.last_seen),
                Col::Created => a.created.cmp(&b.created),
            };
            // newest key breaks ties, so the order never jumps between refreshes
            let o = if self.ascending { o } else { o.reverse() };
            o.then(b.created.cmp(&a.created))
        });
        rows
    }

    fn header(&mut self, ui: &mut egui::Ui, col: Col, name: &str) {
        let clicked = ui.horizontal(|ui| {
            let r = ui.add(egui::Label::new(egui::RichText::new(name).strong()).sense(egui::Sense::click())).on_hover_text("Sort");
            if self.sort == col {
                // drawn, not a glyph: the bundled fonts have no arrow characters
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                let c = rect.center();
                let (tip, base) = if self.ascending { (-4.0, 3.0) } else { (4.0, -3.0) };
                let points = vec![c + egui::vec2(0.0, tip), c + egui::vec2(-4.0, base), c + egui::vec2(4.0, base)];
                ui.painter().add(egui::Shape::convex_polygon(points, ui.visuals().strong_text_color(), egui::Stroke::NONE));
            }
            r.clicked()
        }).inner;
        if clicked {
            if self.sort == col {
                self.ascending = !self.ascending;
            } else {
                self.sort = col;
                // dates read best newest first, names A to Z
                self.ascending = !matches!(col, Col::Seen | Col::Created);
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let modal = |id: &str| egui::Modal::new(egui::Id::unique(id));
        let buttons = |ui: &mut egui::Ui, ok: &str, ok_enabled: bool| -> (bool, bool) {
            ui.add_space(12.0);
            let mut r = (false, false);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                r.0 = ui.add_enabled(ok_enabled, egui::Button::new(ok)).clicked();
                r.1 = ui.button("Cancel").clicked();
            });
            r
        };

        if let Some(form) = &mut self.server_form {
            let resp = modal("server").show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Key server");
                ui.add_space(8.0);
                ui.label("Address");
                ui.add(egui::TextEdit::singleline(&mut form.url).desired_width(f32::INFINITY).hint_text("https://mytune-keys.<name>.workers.dev"));
                ui.add_space(4.0);
                ui.label("Admin secret");
                ui.add(egui::TextEdit::singleline(&mut form.secret).desired_width(f32::INFINITY).password(true));
                buttons(ui, "Save", !form.url.trim().is_empty() && !form.secret.is_empty())
            });
            let (save, cancel) = resp.inner;
            if save && !form.url.trim().starts_with("https://") {
                // the admin secret must never go over plain http; the form stays open
                self.error("The server address must start with https://");
            } else if save {
                let mut c = self.server_form.take().unwrap();
                c.url = c.url.trim().to_owned();
                if let Err(e) = save_config(&c) {
                    self.error(format!("Could not save the settings: {e}"));
                }
                self.cfg = c;
                self.refresh(ctx);
            } else if cancel || resp.should_close() {
                self.server_form = None;
            }
        }

        if let Some(note) = &mut self.new_note {
            let resp = modal("new").show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading("New key");
                ui.add_space(8.0);
                ui.label("Who is it for?");
                let r = ui.add(egui::TextEdit::singleline(note).desired_width(f32::INFINITY).hint_text("Name"));
                r.request_focus();
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let (create, cancel) = buttons(ui, "Create", true);
                (create || enter, cancel)
            });
            let (create, cancel) = resp.inner;
            if create {
                let note = self.new_note.take().unwrap().trim().to_owned();
                self.run(ctx, move |c| match call(c, "POST", "/admin/keys", Some(serde_json::json!({ "note": note }))) {
                    Ok(text) => match serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["key"].as_str().map(str::to_owned)) {
                        Some(k) => Reply::Created(k),
                        None => Reply::Error("Unexpected answer from the server".into()),
                    },
                    Err(e) => Reply::Error(e),
                });
            } else if cancel || resp.should_close() {
                self.new_note = None;
            }
        }

        if let Some(key) = &self.created {
            let resp = modal("created").show(ctx, |ui| {
                ui.heading("Key created");
                ui.add_space(8.0);
                ui.label(egui::RichText::new(key).monospace().size(20.0));
                ui.label(egui::RichText::new("Copied to the clipboard.").weak());
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.button("Done").clicked()).inner
            });
            if resp.inner || resp.should_close() {
                self.created = None;
            }
        }

        if let Some(key) = self.confirm_delete.clone() {
            let resp = modal("delete").show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Delete key?");
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&key).monospace());
                ui.label("Whoever uses it loses MyTune once their token runs out. This cannot be undone.");
                buttons(ui, "Delete", true)
            });
            let (yes, no) = resp.inner;
            if yes {
                self.confirm_delete = None;
                self.action(ctx, "/admin/delete", serde_json::json!({ "key": key }));
            } else if no || resp.should_close() {
                self.confirm_delete = None;
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &root.ctx().clone();
        self.poll(ctx);

        egui::Panel::top("bar").frame(egui::Frame::side_top_panel(root.style()).inner_margin(10)).show(root, |ui| {
            ui.horizontal(|ui| {
                if ui.button("New key").clicked() {
                    self.new_note = Some(String::new());
                }
                if ui.button("Refresh").clicked() {
                    self.refresh(ctx);
                }
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search key, name or device").desired_width(200.0));
                if self.busy > 0 {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Server").on_hover_text(&self.cfg.url).clicked() {
                        self.server_form = Some(self.cfg.clone());
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let used = self.rows.iter().filter(|r| r.device.is_some() && !r.revoked).count();
                let revoked = self.rows.iter().filter(|r| r.revoked).count();
                ui.label(egui::RichText::new(format!("{} keys  ·  {used} in use  ·  {revoked} revoked", self.rows.len())).weak());
                let stale_n = self.rows.iter().filter(|r| stale(r)).count();
                if stale_n > 0 {
                    ui.colored_label(ORANGE, format!("·  {stale_n} not seen for {STALE_DAYS}+ days"));
                }
            });
            if let Some((text, is_error)) = &self.status {
                ui.colored_label(if *is_error { RED } else { egui::Color32::from_gray(170) }, text);
            }
        });

        egui::CentralPanel::default().show(root, |ui| {
            if self.rows.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(egui::RichText::new(if self.busy > 0 { "Loading…" } else { "No keys yet. Click New key to make one." }).weak());
                });
                return;
            }
            egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                egui::Grid::new("keys").striped(true).spacing([20.0, 8.0]).min_row_height(24.0).show(ui, |ui| {
                    for (col, name) in [(Col::Key, "Key"), (Col::Note, "For"), (Col::State, "State"), (Col::Device, "Device"), (Col::Seen, "Last seen"), (Col::Created, "Created")] {
                        self.header(ui, col, name);
                    }
                    ui.end_row();
                    let mut todo: Option<(&'static str, serde_json::Value)> = None;
                    let (mut copied, mut delete) = (None, None);
                    for r in self.visible() {
                        if ui.add(egui::Label::new(egui::RichText::new(&r.key).monospace()).sense(egui::Sense::click()))
                            .on_hover_text("Click to copy").clicked()
                        {
                            copied = Some(r.key.clone());
                        }
                        match r.note.as_deref().filter(|n| !n.is_empty()) {
                            Some(n) => ui.label(n),
                            None => ui.weak("—"),
                        };
                        let (state, color) = state_of(r);
                        badge(ui, state, color);
                        match &r.device {
                            Some(d) => ui.label(r.device_name.as_deref().unwrap_or("Unnamed"))
                                .on_hover_text(format!("Device {d}\nBound {}", r.bound_at.map(date).unwrap_or_default())),
                            None => ui.weak("—"),
                        };
                        let seen = r.last_seen.map(days_ago).unwrap_or_else(|| "never".into());
                        if stale(r) {
                            ui.colored_label(ORANGE, seen);
                        } else {
                            ui.weak(seen);
                        }
                        ui.weak(r.created.map(date).unwrap_or_default());
                        ui.horizontal(|ui| {
                            if r.device.is_some() && ui.small_button("Unbind").on_hover_text("Free the key so another PC can use it").clicked() {
                                todo = Some(("/admin/unbind", serde_json::json!({ "key": r.key })));
                            }
                            let (label, tip) = if r.revoked { ("Allow", "Let this key work again") } else { ("Revoke", "Stop this key from working") };
                            if ui.small_button(label).on_hover_text(tip).clicked() {
                                todo = Some(("/admin/revoke", serde_json::json!({ "key": r.key, "revoked": !r.revoked })));
                            }
                            if ui.small_button(egui::RichText::new("Delete").color(RED)).clicked() {
                                delete = Some(r.key.clone());
                            }
                        });
                        ui.end_row();
                    }
                    if let Some(key) = copied {
                        ctx.copy_text(key.clone());
                        self.info(format!("Copied {key}"));
                    }
                    if delete.is_some() {
                        self.confirm_delete = delete;
                    }
                    if let Some((path, body)) = todo {
                        self.action(ctx, path, body);
                    }
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
