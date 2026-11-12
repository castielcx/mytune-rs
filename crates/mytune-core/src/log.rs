//! Log to `%ProgramData%\MyTune\mytune.log`, same format as C# `Log`:
//! `yyyy-MM-dd HH:mm:ss [INFO|ERROR] msg`, CRLF, UTF-8 with BOM, identical consecutive lines
//! collapsed into one "(previous line repeated N more time[s])", rotated to `.log.old` over 1 MB.
//! Write errors are swallowed on purpose: logging must never break a tweak.

use crate::util::local_stamp;
use std::fmt::Display;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 1_000_000;

pub struct Logger {
    path: PathBuf,
    /// (last "[LEVEL] msg", times repeated and not yet written)
    last: Mutex<(String, u32)>,
}

impl Logger {
    pub fn new(path: PathBuf) -> Logger {
        Logger { path, last: Mutex::new((String::new(), 0)) }
    }

    pub fn info(&self, msg: &str) {
        self.write("INFO", msg);
    }

    /// `msg: error` (C# prints `msg: ExType: message`; Rust errors carry no type name).
    pub fn error(&self, msg: &str, err: Option<&dyn Display>) {
        match err {
            Some(e) => self.write("ERROR", &format!("{msg}: {e}")),
            None => self.write("ERROR", msg),
        }
    }

    fn write(&self, level: &str, msg: &str) {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let key = format!("[{level}] {msg}");
        if last.0 == key {
            last.1 += 1;
            return;
        }
        if last.1 > 0 {
            let n = last.1;
            self.append(&format!("[INFO] (previous line repeated {n} more time{})", if n == 1 { "" } else { "s" }));
        }
        *last = (key.clone(), 0);
        self.append(&key);
    }

    fn append(&self, body: &str) {
        let _ = self.try_append(body);
    }

    fn try_append(&self, body: &str) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        if fs::metadata(&self.path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
            let mut old = self.path.clone().into_os_string();
            old.push(".old");
            fs::rename(&self.path, old)?; // replaces an existing .old on Windows
        }
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let mut buf = Vec::new();
        if f.metadata()?.len() == 0 {
            buf.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        buf.extend_from_slice(format!("{} {body}\r\n", local_stamp()).as_bytes());
        f.write_all(&buf)
    }
}

fn global() -> &'static Logger {
    static G: OnceLock<Logger> = OnceLock::new();
    // Unit tests must never write to the real log.
    #[cfg(test)]
    return G.get_or_init(|| Logger::new(std::env::temp_dir().join("mytune-core-test").join("mytune.log")));
    #[cfg(not(test))]
    G.get_or_init(|| Logger::new(crate::util::default_dir().join("mytune.log")))
}

pub fn info(msg: &str) {
    global().info(msg);
}

pub fn error(msg: &str, err: Option<&dyn Display>) {
    global().error(msg, err);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &std::path::Path) -> String {
        String::from_utf8(fs::read(p).unwrap()).unwrap()
    }

    #[test]
    fn format_bom_and_dedupe() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("t.log");
        let l = Logger::new(p.clone());
        l.info("hello");
        l.info("hello");
        l.info("hello");
        l.error("bad", Some(&"boom"));
        l.info("hello");
        let raw = fs::read(&p).unwrap();
        assert_eq!(&raw[..3], &[0xEF, 0xBB, 0xBF]);
        let text = read(&p);
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').split("\r\n").filter(|s| !s.is_empty()).collect();
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[0].ends_with("[INFO] hello") && lines[0].as_bytes()[4] == b'-' && lines[0].as_bytes()[13] == b':');
        assert!(lines[1].ends_with("[INFO] (previous line repeated 2 more times)"));
        assert!(lines[2].ends_with("[ERROR] bad: boom"));
        assert!(lines[3].ends_with("[INFO] hello"));
        assert_eq!(text.matches('\u{feff}').count(), 1, "BOM only once");
    }

    #[test]
    fn rotates_over_one_mb() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.log");
        fs::write(&p, vec![b'x'; 1_000_001]).unwrap();
        fs::write(d.path().join("t.log.old"), b"older").unwrap();
        Logger::new(p.clone()).info("fresh");
        assert_eq!(fs::metadata(d.path().join("t.log.old")).unwrap().len(), 1_000_001);
        assert!(read(&p).contains("fresh") && fs::metadata(&p).unwrap().len() < 200);
    }
}
