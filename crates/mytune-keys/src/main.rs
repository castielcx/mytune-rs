//! MyTune key manager: creates, revokes, unbinds and deletes licence keys on the key server (keyserver\worker.js).
//! `mytune-keys keygen` prints a fresh Ed25519 key pair instead of opening the window.
//! `mytune-keys update-keygen` creates the release signing key once; `update-sign <setup.exe> <version> <public>`
//! writes the signed version.json next to the setup.
// No console window of its own; the commands attach to the terminal they were started from to print there.
#![windows_subsystem = "windows"]

#[link(name = "kernel32")]
extern "system" {
    fn AttachConsole(process: u32) -> i32;
    fn GetStdHandle(which: u32) -> isize;
    fn GetCurrentProcess() -> isize;
    fn CloseHandle(handle: isize) -> i32;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn CreateFileW(name: *const u16, access: u32, share: u32, security: *const SecurityAttributes, disposition: u32, flags: u32, template: isize) -> isize;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
    fn GetTokenInformation(token: isize, class: u32, info: *mut c_void, size: u32, written: *mut u32) -> i32;
    fn ConvertSidToStringSidW(sid: *const c_void, text: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl: *const u16, revision: u32, descriptor: *mut *mut c_void, size: *mut u32) -> i32;
}

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

use base64::{engine::general_purpose::STANDARD, Engine as _};
use eframe::egui;
use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
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

fn attach_console() {
    // SAFETY: plain Win32 calls. Attach (u32::MAX = parent process) only when stdout (-11) is not
    // already a pipe or file: attaching would replace it and a script would read nothing.
    unsafe {
        if matches!(GetStdHandle(-11i32 as u32), 0 | -1) {
            AttachConsole(u32::MAX);
        }
    }
}

fn update_key_path() -> PathBuf {
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join(".mytune").join("update-signing.key")
}

/// SAFETY: `sid` must point to a valid SID.
unsafe fn sid_text(sid: *const c_void) -> Result<String, String> {
    let mut text = std::ptr::null_mut();
    if ConvertSidToStringSidW(sid, &mut text) == 0 {
        return Err(format!("Cannot read the user SID: {}", std::io::Error::last_os_error()));
    }
    let len = (0..).take_while(|&i| *text.add(i) != 0).count();
    let out = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
    LocalFree(text.cast());
    Ok(out)
}

fn current_user_sid() -> Result<String, String> {
    // SAFETY: the token is closed once; TOKEN_USER starts with a pointer to the SID stored in the same buffer.
    unsafe {
        let mut token = 0isize;
        if OpenProcessToken(GetCurrentProcess(), 8 /* TOKEN_QUERY */, &mut token) == 0 {
            return Err(format!("Cannot open the process token: {}", std::io::Error::last_os_error()));
        }
        let mut info = [0usize; 16];
        let mut written = 0u32;
        let ok = GetTokenInformation(token, 1 /* TokenUser */, info.as_mut_ptr().cast(), std::mem::size_of_val(&info) as u32, &mut written);
        let error = std::io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(format!("Cannot read the current user: {error}"));
        }
        sid_text(info[0] as *const c_void)
    }
}

/// Creates a new file only the current user can open. The DACL is set at creation, so the seed is never in a looser file.
fn create_private_file(path: &Path) -> Result<std::fs::File, String> {
    use std::os::windows::{ffi::OsStrExt as _, io::FromRawHandle as _};
    let sid = current_user_sid()?;
    let sddl: Vec<u16> = format!("O:{sid}D:P(A;;FA;;;{sid})").encode_utf16().chain([0]).collect();
    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: both strings are NUL-terminated, the descriptor outlives CreateFileW, and the handle is owned by the File.
    unsafe {
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()) == 0 {
            return Err(format!("Cannot build the key file permissions: {}", std::io::Error::last_os_error()));
        }
        let security = SecurityAttributes { length: std::mem::size_of::<SecurityAttributes>() as u32, descriptor, inherit: 0 };
        let handle = CreateFileW(name.as_ptr(), 0x4000_0000 /* GENERIC_WRITE */, 0, &security, 1 /* CREATE_NEW */, 0x80 /* NORMAL */, 0);
        let error = std::io::Error::last_os_error();
        LocalFree(descriptor);
        if handle == -1 {
            return Err(format!("Cannot create {} (an existing key is never replaced): {error}", path.display()));
        }
        Ok(std::fs::File::from_raw_handle(handle as *mut c_void))
    }
}

/// Writes a new key once (base64 of the 32-byte seed) and returns the public key.
fn create_update_key(path: &Path) -> Result<String, String> {
    use std::io::Write as _;
    let parent = path.parent().ok_or("The key path has no folder.")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| format!("No system random source: {e}"))?;
    let mut file = create_private_file(path)?;
    // Flushed to disk: a lost key means installed apps can never accept another update.
    if let Err(e) = file.write_all(STANDARD.encode(seed).as_bytes()).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("Cannot write {}: {e}", path.display()));
    }
    Ok(STANDARD.encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes()))
}

/// Must match `signed_bytes` and `URL_PREFIX` in mytune-core's updater.rs; the golden test pins the format.
fn sign_manifest(seed: &[u8; 32], version: &str, sha256: &str) -> serde_json::Value {
    let url = format!("https://github.com/castielcx/mytune-rs/releases/download/v{version}/MyTune-rs-Setup-{version}.exe");
    let sig = SigningKey::from_bytes(seed).sign(format!("mytune-update-v1\n{version}\n{url}\n{sha256}").as_bytes());
    serde_json::json!({ "version": version, "url": url, "sha256": sha256, "sig": STANDARD.encode(sig.to_bytes()) })
}

fn sign_setup(seed: &[u8; 32], expected_public: &str, version: &str, file_name: &str, setup: &[u8]) -> Result<serde_json::Value, String> {
    if STANDARD.encode(SigningKey::from_bytes(seed).verifying_key().to_bytes()) != expected_public {
        return Err("The signing key does not match the expected public key.".into());
    }
    // What parse_version in updater.rs accepts, minus its trimming. Digits and dots also satisfy the app's URL rule.
    let parts: Vec<&str> = version.split('.').collect();
    if !(2..=4).contains(&parts.len()) || !parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u32>().is_ok()) {
        return Err(format!("{version} is not a version the app accepts (2 to 4 numbers separated by dots)."));
    }
    if file_name != format!("MyTune-rs-Setup-{version}.exe") {
        return Err(format!("{file_name} is not the setup of version {version}."));
    }
    Ok(sign_manifest(seed, version, &format!("{:x}", Sha256::digest(setup))))
}

fn update_command(args: &[String]) -> Result<(), String> {
    let key_path = update_key_path();
    // Never a relative path: the private key must not land in the current folder.
    if !key_path.is_absolute() {
        return Err("USERPROFILE is not set.".into());
    }
    match args {
        [cmd] if cmd == "update-keygen" => {
            let public = create_update_key(&key_path)?;
            println!("Key file (back it up, keep it secret): {}", key_path.display());
            println!("UPDATE_PUBLIC_KEY: {public}");
            Ok(())
        }
        [cmd, setup, version, public] if cmd == "update-sign" => {
            let text = std::fs::read_to_string(&key_path).map_err(|e| format!("Cannot read {}: {e}", key_path.display()))?;
            let seed: [u8; 32] = STANDARD.decode(text.trim()).ok().and_then(|b| b.try_into().ok())
                .ok_or_else(|| format!("{} is not a valid update key.", key_path.display()))?;
            let setup = Path::new(setup);
            let file_name = setup.file_name().and_then(|n| n.to_str()).ok_or("The setup path has no file name.")?;
            let bytes = std::fs::read(setup).map_err(|e| format!("Cannot read {}: {e}", setup.display()))?;
            let manifest = sign_setup(&seed, public, version, file_name, &bytes)?;
            let out = setup.with_file_name("version.json");
            let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
            std::fs::write(&out, json).map_err(|e| format!("Cannot write {}: {e}", out.display()))?;
            println!("Wrote {}", out.display());
            Ok(())
        }
        _ => Err("Usage: mytune-keys update-keygen | mytune-keys update-sign <setup.exe> <version> <expected-public-base64>".into()),
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if matches!(args.first().map(String::as_str), Some("update-keygen" | "update-sign")) {
        attach_console();
        if let Err(e) = update_command(&args) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("keygen") {
        attach_console();
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
    #[test] fn sign_refuses_a_name_the_app_url_rule_rejects() {
        assert!(sign_setup(&SEED, PUBLIC, "0.3.0 x", "MyTune-rs-Setup-0.3.0 x.exe", b"abc").is_err());
    }
    #[test] fn sign_accepts_only_versions_the_app_parses() {
        for bad in ["0.3.0-rc1", "1", "1.2.3.4.5", "+1.2", "1..2", "4294967296.0"] {
            assert!(sign_setup(&SEED, PUBLIC, bad, &format!("MyTune-rs-Setup-{bad}.exe"), b"abc").is_err(), "{bad}");
        }
        assert!(sign_setup(&SEED, PUBLIC, "0.3.0", "MyTune-rs-Setup-0.3.0.exe", b"abc").is_ok());
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn GetNamedSecurityInfoW(name: *const u16, object: u32, info: u32, owner: *mut *mut c_void, group: *mut *mut c_void, dacl: *mut *mut u8, sacl: *mut *mut c_void, descriptor: *mut *mut c_void) -> u32;
        fn GetSecurityDescriptorControl(descriptor: *const c_void, control: *mut u16, revision: *mut u32) -> i32;
    }

    #[test] fn update_key_file_is_private() {
        use std::os::windows::ffi::OsStrExt as _;
        use std::ptr::null_mut;
        let dir = std::env::temp_dir().join(format!("mytune-keys-dacl-{}", std::process::id()));
        let path = dir.join("update-signing.key");
        let _ = std::fs::remove_dir_all(&dir);
        create_update_key(&path).unwrap();
        let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: the DACL points into `descriptor`, which is freed last. ACL: AceCount at 4, first ACE at 8.
        // ACCESS_ALLOWED_ACE: type at 0, mask at 4, SID at 8.
        let (control, aces, ace_type, mask, sid) = unsafe {
            let (mut dacl, mut descriptor) = (null_mut::<u8>(), null_mut());
            assert_eq!(GetNamedSecurityInfoW(name.as_ptr(), 1 /* file */, 4 /* DACL */, null_mut(), null_mut(), &mut dacl, null_mut(), &mut descriptor), 0);
            let (mut control, mut revision) = (0u16, 0u32);
            assert_ne!(GetSecurityDescriptorControl(descriptor, &mut control, &mut revision), 0);
            let ace = dacl.add(8);
            let read = (control, dacl.add(4).cast::<u16>().read_unaligned(), *ace, ace.add(4).cast::<u32>().read_unaligned(), sid_text(ace.add(8).cast()).unwrap());
            LocalFree(descriptor);
            read
        };
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(control & 0x1000, 0x1000, "DACL is not protected");
        assert_eq!(aces, 1);
        assert_eq!((ace_type, mask), (0, 0x1F01FF), "not one full-access allow entry");
        assert_eq!(sid, current_user_sid().unwrap());
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
}
