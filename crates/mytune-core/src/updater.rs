//! Update check: reads the signed `version.json` of the latest mytune-rs release and says whether a newer
//! MyTune exists. Nothing about the user is sent; it is a plain download. Only with the `license` feature.

/// Version of this build. `sysreport` prints it and the manifest is compared against it.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(feature = "license")]
pub use enabled::*;

#[cfg(feature = "license")]
mod enabled {
    use super::APP_VERSION;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    use ed25519_dalek::{Signature, VerifyingKey};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::fs::{self, File};
    use std::io::{Read, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    pub const FEED_URL: &str = "https://github.com/castielcx/mytune-rs/releases/latest/download/version.json";
    pub const URL_PREFIX: &str = "https://github.com/castielcx/mytune-rs/releases/download/";

    /// Public half of the owner's update key. Its private half stays in `%USERPROFILE%\.mytune\update-signing.key`.
    const UPDATE_PUBLIC_KEY: &str = "m7oWKknt+PmQYeVOcWGPg37GJ/F6QOe/gHLAiCJQJ1I=";
    const _: () = assert!(cfg!(debug_assertions) || UPDATE_PUBLIC_KEY.len() == 44, "UPDATE_PUBLIC_KEY is still the placeholder: run the key step in the forced-update plan");

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UpdateInfo {
        pub version: String,
        pub url: String,
        pub sha256: String,
    }

    #[derive(Deserialize)]
    struct Feed {
        version: Option<String>,
        url: Option<String>,
        sha256: Option<String>,
        sig: Option<String>,
    }

    /// The exact bytes the release signature covers.
    pub fn signed_bytes(version: &str, url: &str, sha256: &str) -> Vec<u8> {
        format!("mytune-update-v1\n{version}\n{url}\n{sha256}").into_bytes()
    }

    /// "2.4.3" -> (2, 4, 3); missing parts are 0 like .NET `Version` comparisons of 3-part numbers.
    pub fn parse_version(v: &str) -> Option<Vec<u32>> {
        let parts: Option<Vec<u32>> = v.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.parse().ok()).collect();
        parts.filter(|p| (2..=4).contains(&p.len()))
    }

    fn newer(latest: &[u32], current: &[u32]) -> bool {
        let n = latest.len().max(current.len());
        let at = |v: &[u32], i: usize| v.get(i).copied().unwrap_or(0);
        (0..n).map(|i| (at(latest, i), at(current, i))).find(|(a, b)| a != b).is_some_and(|(a, b)| a > b)
    }

    fn parse_key(b64: &str) -> Result<VerifyingKey, String> {
        let raw: [u8; 32] = STANDARD.decode(b64).ok().and_then(|b| b.try_into().ok()).ok_or("This build has no valid update key.")?;
        VerifyingKey::from_bytes(&raw).map_err(|_| "This build has no valid update key.".to_string())
    }

    /// Exactly `<tag>/<asset>` after the prefix. An allow-list, because the URL parser resolves encoded dot
    /// segments (`%2e%2e`) and strips tabs and newlines, so a list of forbidden characters can be bypassed.
    fn url_allowed(url: &str) -> bool {
        let segment_ok = |s: &str| !matches!(s, "" | "." | "..") && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        url.strip_prefix(URL_PREFIX).and_then(|rest| rest.split_once('/')).is_some_and(|(tag, asset)| segment_ok(tag) && segment_ok(asset))
    }

    /// Validates the manifest. `Ok(Some)` = newer version, `Ok(None)` = up to date. Nothing is trusted before
    /// the signature check, and the signature covers the raw strings, before any normalising.
    pub fn evaluate(feed_json: &str, current: &str, key: &VerifyingKey) -> Result<Option<UpdateInfo>, String> {
        // the release script writes version.json with PowerShell, which puts a UTF-8 BOM first
        let feed: Feed = serde_json::from_str(feed_json.trim_start_matches('\u{feff}')).map_err(|_| "The update manifest is not valid.".to_string())?;
        let (Some(version), Some(url), Some(sha256), Some(sig)) = (feed.version, feed.url, feed.sha256, feed.sig) else {
            return Err("The update manifest is incomplete.".into());
        };
        let sig = STANDARD.decode(&sig).ok().and_then(|b| Signature::from_slice(&b).ok()).ok_or("The update signature is not valid.")?;
        key.verify_strict(&signed_bytes(&version, &url, &sha256), &sig).map_err(|_| "The update signature is not valid.".to_string())?;
        if !url_allowed(&url) {
            return Err("The update download address is not allowed.".into());
        }
        if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("The update has no valid SHA-256.".into());
        }
        let latest = parse_version(&version).ok_or("The update has no valid version.")?;
        let cur = parse_version(current).ok_or("Bad current version.")?;
        Ok(newer(&latest, &cur).then(|| UpdateInfo { version, url, sha256: sha256.to_lowercase() }))
    }

    fn feed_url() -> String {
        #[cfg(debug_assertions)]
        if let Ok(url) = std::env::var("MYTUNE_TEST_FEED") {
            return url;
        }
        FEED_URL.to_string()
    }

    fn current_version() -> String {
        #[cfg(debug_assertions)]
        if let Ok(version) = std::env::var("MYTUNE_TEST_VERSION") {
            return version;
        }
        APP_VERSION.to_string()
    }

    /// Downloads the manifest (15 s) and compares it with this version.
    fn fetch() -> Result<Option<UpdateInfo>, String> {
        let key = parse_key(UPDATE_PUBLIC_KEY)?;
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
        let text = agent.get(&feed_url()).call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
        evaluate(&text, &current_version(), &key)
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum Phase {
        Ready,
        Downloading { got: u64, total: Option<u64> },
        Installing,
        Failed(String),
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct Pending {
        pub info: UpdateInfo,
        pub phase: Phase,
        pub blocks: bool,
    }

    static STATE: Mutex<Option<Pending>> = Mutex::new(None);
    static REPAINT: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    /// Cheap enough for every frame.
    pub fn pending() -> Option<Pending> {
        STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Read and write under one lock, so a check cannot overwrite a phase change made meanwhile.
    fn update(f: impl FnOnce(Option<Pending>) -> Option<Pending>) {
        let changed = {
            let mut now = STATE.lock().unwrap_or_else(|e| e.into_inner());
            let next = f(now.clone());
            let changed = *now != next;
            *now = next;
            changed
        };
        if changed {
            if let Some(repaint) = REPAINT.get() {
                repaint();
            }
        }
    }

    fn set(next: Option<Pending>) {
        update(|_| next);
    }

    /// The block becomes a banner for this session. The next start blocks again.
    pub fn demote() {
        update(|now| now.map(|p| Pending { blocks: false, ..p }));
    }

    #[doc(hidden)]
    pub fn fake_pending(p: Option<Pending>) {
        set(p);
    }

    /// A running install is never touched, a block is never lifted here, and only a newer version replaces
    /// the one on offer.
    fn merge(now: Option<Pending>, found: UpdateInfo, blocks: bool, failed: bool) -> Option<Pending> {
        let phase = if failed { Phase::Failed("The last update did not finish.".into()) } else { Phase::Ready };
        let Some(p) = now else { return Some(Pending { info: found, phase, blocks }) };
        if matches!(p.phase, Phase::Downloading { .. } | Phase::Installing) {
            return Some(p);
        }
        let blocks = p.blocks || blocks;
        let is_newer = parse_version(&found.version).zip(parse_version(&p.info.version)).is_some_and(|(a, b)| newer(&a, &b));
        Some(if is_newer {
            Pending { info: found, phase, blocks }
        } else {
            Pending { phase: if failed { phase } else { p.phase }, info: p.info, blocks }
        })
    }

    fn check_as(blocks: bool, failed: bool) -> Result<Option<UpdateInfo>, String> {
        let found = fetch()?;
        if let Some(info) = &found {
            update(|now| merge(now, info.clone(), blocks, failed));
        }
        Ok(found)
    }

    /// A check from the Settings page: a version it finds is offered, never forced.
    pub fn check() -> Result<Option<UpdateInfo>, String> {
        check_as(false, false)
    }

    /// Logs a failed check and changes nothing, so an offline PC or a bad feed never locks anyone out.
    fn check_logged(blocks: bool, failed: bool) -> bool {
        check_as(blocks, failed).map_err(|e| crate::log::error("Update check failed", Some(&e))).is_ok()
    }

    /// Starts the one background check thread; later calls do nothing. It sleeps between checks.
    pub fn start(update_failed: bool, repaint: impl Fn() + Send + Sync + 'static) {
        if REPAINT.set(Box::new(repaint)).is_err() {
            return;
        }
        let thread = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let cleaned = updates_dir().and_then(|dir| {
                if fs::symlink_metadata(&dir).is_err() {
                    return Ok(());
                }
                crate::store::secure_private_dir(&dir).map_err(|e| e.to_string())
            });
            if let Err(e) = cleaned {
                crate::log::error("Old update files not removed", Some(&e));
            }
            if !check_logged(true, update_failed) {
                std::thread::sleep(Duration::from_secs(60));
                check_logged(true, update_failed);
            }
            loop {
                std::thread::sleep(Duration::from_secs(24 * 3600));
                check_logged(false, false);
            }
        });
        if let Err(e) = thread {
            crate::log::error("Update check thread not started", Some(&e));
        }
    }

    const SETUP_ARGS: [&str; 4] = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH"];
    const MAX_SETUP_BYTES: u64 = 512 * 1024 * 1024;

    /// `Program Files\MyTune Updates`. This process is elevated and runs what is in here, so not under
    /// `%ProgramData%`, where a standard user can pre-create and own `MyTune`, and never from an environment
    /// variable, which a non-admin can set through `HKCU\Environment`.
    fn updates_dir() -> Result<PathBuf, String> {
        let base = crate::util::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles).map_err(|e| format!("The update folder was not found: {e}"))?;
        Ok(base.join("MyTune Updates"))
    }

    /// `create_new`, so it never writes through a file or link already at `dest`. Read timeout only: a total
    /// timeout would cut off a slow but working download.
    fn download(url: &str, dest: &Path, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<(), String> {
        let mut out = fs::OpenOptions::new().write(true).create_new(true).open(dest).map_err(|e| e.to_string())?;
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(30)).build();
        let reply = agent.get(url).call().map_err(|e| e.to_string())?;
        let total = reply.header("Content-Length").and_then(|v| v.parse().ok());
        let mut body = reply.into_reader();
        let (mut buf, mut got) = (vec![0; 64 * 1024], 0u64);
        loop {
            let n = body.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                return out.sync_all().map_err(|e| e.to_string());
            }
            got += n as u64;
            if got > MAX_SETUP_BYTES {
                return Err("The download is too large.".into());
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            progress(got, total);
        }
    }

    /// The handle allows only other readers, so the file cannot change between this check and the start.
    fn open_verified(path: &Path, sha256: &str) -> Result<File, String> {
        const FILE_SHARE_READ: u32 = 1;
        let mut file = fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
        let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if hash != sha256 {
            return Err("The download was damaged.".into());
        }
        Ok(file)
    }

    /// `held` closes only after the process exists, so the verified file cannot be swapped before the start.
    fn run_setup(path: &Path, held: File, args: &[&str]) -> Result<i32, String> {
        let mut child = Command::new(path).args(args).spawn().map_err(|e| format!("Setup could not start: {e}"))?;
        drop(held);
        child.wait().map_err(|e| e.to_string())?.code().ok_or_else(|| "Setup was stopped.".to_string())
    }

    /// Setup closes MyTune when it works, so any exit seen from here is a failure, even code 0.
    fn exit_text(code: i32) -> String {
        if code == 0 {
            "Setup ended without replacing MyTune.".into()
        } else {
            format!("Setup stopped with code {code}. Antivirus may have blocked it.")
        }
    }

    /// Keeps `blocks`, which `demote` can change at any moment.
    fn set_phase(phase: Phase) {
        update(|now| now.map(|p| Pending { phase, ..p }));
    }

    /// Returns the exit code of a setup that ended while MyTune is still alive.
    fn install_steps(info: &UpdateInfo) -> Result<i32, String> {
        let dir = updates_dir()?;
        crate::store::secure_private_dir(&dir).map_err(|e| format!("The update folder could not be prepared: {e}"))?;
        let path = dir.join("MyTune-Setup.exe");
        let mut shown = u64::MAX;
        download(&info.url, &path, &mut |got, total| {
            // whole percent, or whole MiB when the size is unknown
            let step = total.filter(|t| *t > 0).map_or(got >> 20, |t| got * 100 / t);
            if step != shown {
                shown = step;
                set_phase(Phase::Downloading { got, total });
            }
        })?;
        let held = open_verified(&path, &info.sha256).inspect_err(|_| {
            let _ = fs::remove_file(&path);
        })?;
        set_phase(Phase::Installing);
        run_setup(&path, held, &SETUP_ARGS)
    }

    /// Downloads, verifies and starts the setup on its own thread. Does nothing while an install runs.
    pub fn install() {
        let mut claimed = None;
        // claimed under the lock, so two clicks cannot start two installs
        update(|now| {
            let p = now?;
            if matches!(p.phase, Phase::Downloading { .. } | Phase::Installing) {
                return Some(p);
            }
            claimed = Some(p.info.clone());
            Some(Pending { phase: Phase::Downloading { got: 0, total: None }, ..p })
        });
        let Some(info) = claimed else { return };
        let thread = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            let reason = install_steps(&info).map_or_else(|e| e, exit_text);
            crate::log::error("Update failed", Some(&reason));
            set_phase(Phase::Failed(reason));
        });
        if let Err(e) = thread {
            crate::log::error("Update thread not started", Some(&e));
            set_phase(Phase::Failed("The update could not start.".into()));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ed25519_dalek::{Signer, SigningKey};

        const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        const URL: &str = "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/MyTune-rs-Setup-0.3.0.exe";
        // Made outside this code from the seed [7; 32]; mytune-keys tests the same pair (Task 6).
        const GOLDEN_PUB: &str = "6kpsY+KcUgq+9VB7Ey7F+ZVHdq6+vnuSQh7qaRRG0iw=";
        const GOLDEN_SIG: &str = "vWiBcPgW3uydpuscL6+P0xr/1BaPYDVEsbA5gFX6gr1p2974j9bdVPpUCWH5eYqzL9pQhLQnOTunc4L79/5eAA==";

        fn sk() -> SigningKey { SigningKey::from_bytes(&[7; 32]) }
        fn feed(k: &SigningKey, v: &str, url: &str, sha: &str) -> String {
            let sig = STANDARD.encode(k.sign(&signed_bytes(v, url, sha)).to_bytes());
            serde_json::json!({ "version": v, "url": url, "sha256": sha, "sig": sig }).to_string()
        }

        #[test] fn golden_vector_verifies() {
            let j = serde_json::json!({ "version": "0.3.0", "url": URL, "sha256": SHA, "sig": GOLDEN_SIG }).to_string();
            assert_eq!(evaluate(&j, "0.2.0", &parse_key(GOLDEN_PUB).unwrap()).unwrap().unwrap().version, "0.3.0");
        }
        #[test] fn valid_newer_manifest_is_accepted() {
            let u = evaluate(&feed(&sk(), "0.3.0", URL, SHA), "0.2.0", &sk().verifying_key()).unwrap().unwrap();
            assert_eq!((u.url.as_str(), u.sha256.as_str()), (URL, SHA));
        }
        #[test] fn bom_is_accepted() {
            assert!(evaluate(&format!("\u{feff}{}", feed(&sk(), "0.3.0", URL, SHA)), "0.2.0", &sk().verifying_key()).unwrap().is_some());
        }
        #[test] fn wrong_key_is_refused() {
            let other = SigningKey::from_bytes(&[8; 32]);
            assert!(evaluate(&feed(&other, "0.3.0", URL, SHA), "0.2.0", &sk().verifying_key()).is_err());
        }
        #[test] fn each_changed_field_is_refused() {
            let good = feed(&sk(), "0.3.0", URL, SHA);
            for (from, to) in [("0.3.0\"", "0.4.0\""), ("v0.3.0/", "v0.4.0/"), ("01234567", "11234567")] {
                assert!(evaluate(&good.replacen(from, to, 1), "0.2.0", &sk().verifying_key()).is_err(), "{from}");
            }
        }
        #[test] fn bad_url_is_refused() { // correctly signed, so only the URL rule can stop them
            for url in [
                "http://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s.exe",
                "https://github.com/castielcx/mytune-rs-evil/releases/download/v0.3.0/s.exe",
                "https://github.com/other/mytune-rs/releases/download/v0.3.0/s.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/../../../../evil/s.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s.exe?x=1",
                "https://github.com/castielcx/mytune-rs/releases/download/%2e%2e/%2e%2e/%2e%2e/%2e%2e/evil/repo/s.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/.%2E/s.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s\t.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/sub/s.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/",
                "https://github.com/castielcx/mytune-rs/releases/download/./s.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s.exe#x",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s\\x.exe",
                "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/s@x.exe",
            ] {
                assert!(evaluate(&feed(&sk(), "0.3.0", url, SHA), "0.2.0", &sk().verifying_key()).is_err(), "{url}");
            }
        }
        #[test] fn bad_hash_is_refused() {
            for sha in ["abc", &"g".repeat(64), &"a".repeat(65)] {
                assert!(evaluate(&feed(&sk(), "0.3.0", URL, sha), "0.2.0", &sk().verifying_key()).is_err(), "{sha}");
            }
        }
        #[test] fn same_and_older_version_are_up_to_date() {
            for v in ["0.2.0", "0.1.9"] {
                assert_eq!(evaluate(&feed(&sk(), v, URL, SHA), "0.2.0", &sk().verifying_key()), Ok(None), "{v}");
            }
        }
        #[test] fn garbage_is_an_error() {
            for text in ["", "not json", "{}", r#"{"version":"0.3.0"}"#] {
                assert!(evaluate(text, "0.2.0", &sk().verifying_key()).is_err(), "{text}");
            }
        }
        #[test] fn empty_key_is_refused() { assert!(parse_key("").is_err()); }
        #[test] fn embedded_key_is_valid_or_the_placeholder() {
            assert!(UPDATE_PUBLIC_KEY.is_empty() || parse_key(UPDATE_PUBLIC_KEY).is_ok());
        }

        fn info(v: &str) -> UpdateInfo { UpdateInfo { version: v.into(), url: URL.into(), sha256: SHA.into() } }
        fn at(v: &str, phase: Phase, blocks: bool) -> Option<Pending> { Some(Pending { info: info(v), phase, blocks }) }

        #[test] fn start_check_blocks() { assert_eq!(merge(None, info("0.3.0"), true, false), at("0.3.0", Phase::Ready, true)); }
        #[test] fn later_check_only_offers() { assert_eq!(merge(None, info("0.3.0"), false, false), at("0.3.0", Phase::Ready, false)); }
        #[test] fn update_failed_start_shows_failure() {
            let p = merge(None, info("0.3.0"), true, true).unwrap();
            assert!(p.blocks && matches!(p.phase, Phase::Failed(_)));
        }
        #[test] fn running_install_is_not_replaced() {
            for phase in [Phase::Downloading { got: 1, total: None }, Phase::Installing] {
                let now = at("0.3.0", phase, true);
                assert_eq!(merge(now.clone(), info("0.4.0"), false, false), now);
            }
        }
        #[test] fn same_version_keeps_phase() {
            let now = at("0.3.0", Phase::Failed("x".into()), true);
            assert_eq!(merge(now.clone(), info("0.3.0"), false, false), now);
        }
        #[test] fn newer_version_replaces_failure_and_keeps_block() {
            assert_eq!(merge(at("0.3.0", Phase::Failed("x".into()), true), info("0.3.1"), false, false), at("0.3.1", Phase::Ready, true));
        }
        #[test] fn older_version_is_ignored() {
            let now = at("0.3.1", Phase::Ready, false);
            assert_eq!(merge(now.clone(), info("0.3.0"), false, false), now);
        }
        // "Check now" can record the version before the start check returns; the start check must still block
        #[test] fn start_check_blocks_a_version_already_offered() {
            assert_eq!(merge(at("0.3.0", Phase::Ready, false), info("0.3.0"), true, false), at("0.3.0", Phase::Ready, true));
            let p = merge(at("0.3.0", Phase::Ready, false), info("0.3.0"), true, true).unwrap();
            assert!(p.blocks && matches!(p.phase, Phase::Failed(_)));
        }
        // the only test that touches the global state, so it cannot race
        #[test] fn demote_turns_block_into_banner() {
            fake_pending(at("0.3.0", Phase::Ready, true));
            demote();
            assert!(!pending().unwrap().blocks);
            fake_pending(None);
        }

        const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

        /// Answers one request on localhost with `reply`, then closes.
        fn serve_once(reply: &'static [u8]) -> String {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/s.exe", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let (mut req, mut buf) = (Vec::new(), [0; 1024]);
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(n) if n > 0 => req.extend_from_slice(&buf[..n]),
                        _ => return,
                    }
                }
                let _ = s.write_all(reply);
            });
            url
        }

        #[test] fn download_saves_the_body() {
            let d = tempfile::tempdir().unwrap();
            let dest = d.path().join("s.exe");
            download(&serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"), &dest, &mut |_, _| {}).unwrap();
            assert_eq!(std::fs::read(dest).unwrap(), b"abc");
        }
        #[test] fn truncated_download_is_an_error() {
            let d = tempfile::tempdir().unwrap();
            let r = download(&serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc"), &d.path().join("s.exe"), &mut |_, _| {});
            assert!(r.is_err());
        }
        #[test] fn download_does_not_write_through_an_existing_file() {
            let d = tempfile::tempdir().unwrap();
            let dest = d.path().join("s.exe");
            std::fs::write(&dest, "old").unwrap();
            assert!(download(&serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc"), &dest, &mut |_, _| {}).is_err());
            assert_eq!(std::fs::read(dest).unwrap(), b"old");
        }
        #[test] fn open_verified_refuses_a_wrong_hash() {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("s.exe");
            std::fs::write(&p, "abc").unwrap();
            assert!(open_verified(&p, SHA).is_err());
            assert!(open_verified(&p, ABC_SHA).is_ok());
        }
        #[test] fn verified_file_cannot_be_written_while_held() {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("s.exe");
            std::fs::write(&p, "abc").unwrap();
            let _held = open_verified(&p, ABC_SHA).unwrap();
            assert_eq!(std::fs::OpenOptions::new().write(true).open(&p).unwrap_err().raw_os_error(), Some(32));
        }
        #[test] fn held_setup_still_starts() {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("s.exe");
            std::fs::copy(std::env::var("ComSpec").unwrap(), &p).unwrap();
            let sha: String = Sha256::digest(std::fs::read(&p).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(run_setup(&p, open_verified(&p, &sha).unwrap(), &["/c", "exit 7"]), Ok(7));
        }
        #[test] fn setup_exit_while_alive_is_a_failure() {
            assert!(!exit_text(0).is_empty());
            assert!(exit_text(7).contains('7'));
        }
        // Setting the variables here would race other tests, so a run with them faked proves it (see the Task 3 report).
        #[test] fn updates_dir_ignores_the_environment() {
            let got = updates_dir().unwrap();
            assert!(got.ends_with("MyTune Updates"));
            let known = got.parent().unwrap();
            for name in ["MYTUNE_DIR", "ProgramData", "ProgramFiles", "ProgramW6432"] {
                if let Some(v) = std::env::var_os(name).filter(|v| Path::new(v) != known) {
                    assert!(!got.starts_with(&v), "{name} was used");
                }
            }
        }
    }
}
