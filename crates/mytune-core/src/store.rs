//! `state.json` in `%ProgramData%\MyTune`, shared with the C# app.
//!
//! Only `Applied` (tweak id -> `TweakBackup`) is typed. Every other top-level key (Profiles,
//! Settings, Games, Benchmarks, ...) is kept as raw JSON and written back unchanged, so a Rust
//! save never drops C# data. Keys are PascalCase like the C# file.
//!
//! Save: write `state.json.tmp` (fsync), then `ReplaceFileW` onto `state.json` with `state.json.bak`
//! as the previous generation (atomic, same as C# `File.Replace`). 3 attempts, 100 ms apart.
//! Load: `state.json`; if missing -> `.bak`; if it does not parse -> moved to `.corrupt-<yyyyMMddHHmmss>`, then `.bak`.
//! If it exists but cannot be read right now (locked, no access) it is left alone, nothing is loaded from `.bak`,
//! and every save fails until it can be read (`ensure_read`).

use crate::backup::TweakBackup;
use crate::engine::PENDING;
use crate::error::{Error, Result};
use crate::util::{default_dir, local_compact};
use crate::log;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetKernelObjectSecurity, LookupPrivilegeValueW, SetKernelObjectSecurity, DACL_SECURITY_INFORMATION, OBJECT_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_PRIVILEGES_ATTRIBUTES,
};
use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, ReplaceFileW, BY_HANDLE_FILE_INFORMATION, REPLACE_FILE_FLAGS};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    #[serde(rename = "Applied", default, deserialize_with = "crate::backup::null_default")]
    applied: BTreeMap<String, TweakBackup>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

enum Read {
    Missing,
    /// Read, but not valid state JSON.
    Bad,
    /// Exists, but the read failed (locked, no access): says nothing about its content.
    Unreadable,
    Ok(State),
}

const UNREADABLE: &str = "MyTune can't read its backup file (state.json) right now, so nothing was changed. Try again in a moment.";

pub struct Store {
    dir: PathBuf,
    state: State,
    /// The state as this process last read or wrote it. `save` compares with it to tell this process's own
    /// changes from those of another MyTune writing the same file.
    base: std::sync::Mutex<Value>,
    /// state.json could not be read at load, so its backups are not in `state`.
    unread: bool,
}

/// `theirs` (the file as another process left it) with this process's changes laid over it: every key where
/// `mine` differs from `base` is this process's doing (set, changed or removed), everything else is theirs.
fn merge(base: &Value, mine: &Value, theirs: Value) -> Value {
    match (base, mine, theirs) {
        (Value::Object(b), Value::Object(m), Value::Object(mut t)) => {
            for k in b.keys().chain(m.keys()).collect::<std::collections::BTreeSet<_>>() {
                match (b.get(k), m.get(k)) {
                    (was, now) if was == now => {}
                    (_, None) => {
                        t.remove(k);
                    }
                    (was, Some(now)) => {
                        let v = match (was, t.remove(k)) {
                            (Some(was), Some(their)) => merge(was, now, their),
                            // both sides created this object: theirs keeps the keys mine does not set
                            (None, Some(their)) if now.is_object() && their.is_object() => merge(&Value::Object(Map::new()), now, their),
                            _ => now.clone(),
                        };
                        t.insert(k.clone(), v);
                    }
                }
            }
            Value::Object(t)
        }
        (_, mine, _) => mine.clone(),
    }
}

/// `merge`, except that a backup both sides created stays as it is on disk: it was captured first, so it holds the
/// true originals, while the later capture read values the first apply had already changed.
fn merge_state(base: &Value, mut mine: Value, theirs: Value) -> Value {
    if let (Some(t), Some(m)) = (theirs.get("Applied").and_then(Value::as_object), mine.get_mut("Applied").and_then(Value::as_object_mut)) {
        for (id, b) in t {
            if base["Applied"].get(id).is_none() && m.contains_key(id) {
                m.insert(id.clone(), b.clone());
            }
        }
    }
    merge(base, &mine, theirs)
}

fn read_state(p: &Path) -> Read {
    let mut bytes = None;
    for attempt in 0..10 {
        match fs::read(p) {
            Ok(b) => {
                bytes = Some(b);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
            // 32/33 = file in use by another process (the C# app may be saving)
            Err(e) if matches!(e.raw_os_error(), Some(32) | Some(33)) && attempt < 9 => sleep(Duration::from_millis(300)),
            Err(_) => return Read::Unreadable,
        }
    }
    let Some(b) = bytes else { return Read::Unreadable };
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b);
    match serde_json::from_slice::<State>(b) {
        Ok(s) => Read::Ok(s),
        Err(_) => Read::Bad,
    }
}

fn sibling(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Move (not copy): the next save would otherwise rotate the bad file over the good .bak.
fn quarantine(path: &Path) {
    let corrupt = sibling(path, &format!(".corrupt-{}", local_compact()));
    if fs::rename(path, &corrupt).is_err() {
        let _ = fs::copy(path, &corrupt);
    }
    log::error(&format!("state.json is not valid, kept as {}", corrupt.display()), None);
}

/// The saved state, or `None` while a file that exists cannot be read or no good copy is left (a save would then
/// write an empty state over the backups).
fn load_state(path: &Path) -> Option<State> {
    let from_bak = || match read_state(&sibling(path, ".bak")) {
        Read::Ok(s) => {
            log::info("loaded state.json.bak");
            Some(s)
        }
        Read::Unreadable | Read::Bad => None,
        Read::Missing => Some(State::default()),
    };
    match read_state(path) {
        Read::Ok(s) => Some(s),
        Read::Unreadable => None,
        Read::Missing => from_bak(),
        Read::Bad => {
            quarantine(path);
            from_bak()
        }
    }
}

impl Store {
    pub fn load_default() -> Store {
        Store::load(&default_dir())
    }

    pub fn load(dir: &Path) -> Store {
        let loaded = load_state(&dir.join("state.json"));
        let unread = loaded.is_none();
        if unread {
            log::error("state.json can't be read right now: left as it is, nothing is saved until it can be", None);
        }
        let state = loaded.unwrap_or_default();
        let base = serde_json::to_value(&state).unwrap_or(Value::Null);
        Store { dir: dir.to_path_buf(), state, base: std::sync::Mutex::new(base), unread }
    }

    /// Reads a state.json that could not be read at load, keeping what changed in memory since. Errors while it
    /// still cannot be read: its backups are not in memory then, so nothing may be changed or saved.
    pub fn ensure_read(&mut self) -> Result<()> {
        if !self.unread {
            return Ok(());
        }
        let Some(theirs) = load_state(&self.dir.join("state.json")) else {
            return Err(Error::Plain(UNREADABLE.into()));
        };
        let theirs = serde_json::to_value(&theirs)?;
        let base = self.base.get_mut().unwrap_or_else(|e| e.into_inner());
        self.state = serde_json::from_value(merge(base, &serde_json::to_value(&self.state)?, theirs.clone()))?;
        *base = theirs;
        self.unread = false;
        log::info("state.json can be read again, loaded");
        Ok(())
    }

    /// Brings in what another MyTune saved since this one read the file, keeping unsaved changes made here. Run
    /// before an apply or revert: with a stale copy it would capture tweaked values or miss the backup.
    pub(crate) fn refresh(&mut self) -> Result<()> {
        self.ensure_read()?;
        let path = self.dir.join("state.json");
        if !path.exists() {
            return Ok(());
        }
        let _lock = self.save_lock()?;
        let theirs = match read_state(&path) {
            Read::Ok(t) => serde_json::to_value(&t)?,
            Read::Unreadable => return Err(Error::Plain(UNREADABLE.into())),
            Read::Bad | Read::Missing => return Ok(()), // the next save deals with it
        };
        let mine = serde_json::to_value(&self.state)?;
        let base = self.base.get_mut().unwrap_or_else(|e| e.into_inner());
        self.state = serde_json::from_value(merge_state(base, mine, theirs.clone()))?;
        *base = theirs;
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get_backup(&self, id: &str) -> Option<&TweakBackup> {
        self.state.applied.get(id)
    }

    pub fn applied_ids(&self) -> Vec<String> {
        self.state.applied.keys().cloned().collect()
    }

    /// Tweaks whose failed apply could not be undone: part of the change is still on the PC.
    pub fn partly_applied_ids(&self) -> Vec<String> {
        self.state.applied.iter().filter(|(_, b)| b.partly_applied).map(|(id, _)| id.clone()).collect()
    }

    pub fn set_backup(&mut self, id: &str, b: TweakBackup) -> Result<()> {
        self.state.applied.insert(id.to_string(), b);
        self.save()
    }

    pub fn remove_backup(&mut self, id: &str) -> Result<()> {
        if let Some(old) = self.state.applied.remove(id) {
            if let Err(e) = self.save() {
                // still on disk: dropping it here would make a retry write defaults over the originals
                self.state.applied.insert(id.to_string(), old);
                return Err(e);
            }
        }
        Ok(())
    }

    /// In memory only: puts back what `Engine::begin` replaced after its save failed.
    pub(crate) fn reset_backup(&mut self, id: &str, prev: Option<TweakBackup>) {
        match prev {
            Some(b) => {
                self.state.applied.insert(id.to_string(), b);
            }
            None => {
                self.state.applied.remove(id);
            }
        }
    }

    /// Raw JSON of a key this crate does not own (for tests and tools).
    pub fn other(&self, key: &str) -> Option<&Value> {
        self.state.other.get(key)
    }

    /// Any top-level key the C# app owns (`Games`, `QuietActive`, `PowerPrevious`, ...), as raw JSON.
    pub fn top(&self, key: &str) -> Option<&Value> {
        self.state.other.get(key)
    }

    /// Replaces a top-level key (in memory; call `save`). `Applied` is not reachable this way.
    pub fn set_top(&mut self, key: &str, value: Value) {
        if key != "Applied" {
            self.state.other.insert(key.to_string(), value);
        }
    }

    /// Removes a top-level key (in memory; call `save`).
    pub fn remove_top(&mut self, key: &str) {
        self.state.other.remove(key);
    }

    /// A boolean in the C# `Settings` object (e.g. `GameTimer`); `None` if missing or not a bool.
    pub fn settings_flag(&self, name: &str) -> Option<bool> {
        self.state.other.get("Settings")?.get(name)?.as_bool()
    }

    /// Sets a boolean in `Settings` (creating the object if needed) and saves. All other keys stay as they are.
    pub fn set_settings_flag(&mut self, name: &str, value: bool) -> Result<()> {
        let settings = self.state.other.entry("Settings").or_insert_with(|| Value::Object(Map::new()));
        match settings {
            Value::Object(m) => {
                m.insert(name.to_string(), Value::Bool(value));
            }
            other => *other = serde_json::json!({ name: value }),
        }
        self.save()
    }

    /// A string in `Settings` (e.g. `AccentColor`); `None` if missing or not a string.
    pub fn settings_str(&self, name: &str) -> Option<String> {
        self.state.other.get("Settings")?.get(name)?.as_str().map(String::from)
    }

    /// Sets a string in `Settings` and saves (e.g. `AccentColor`).
    pub fn set_settings_str(&mut self, name: &str, value: &str) -> Result<()> {
        self.set_settings_value(name, Value::String(value.to_string()));
        self.save()
    }

    /// Sets any value in `Settings` (creating the object if needed) without saving. Other keys stay as they are.
    pub fn set_settings_value(&mut self, name: &str, value: Value) {
        let settings = self.state.other.entry("Settings").or_insert_with(|| Value::Object(Map::new()));
        match settings {
            Value::Object(m) => {
                m.insert(name.to_string(), value);
            }
            other => *other = serde_json::json!({ name: value }),
        }
    }

    /// Held from the read to the write of `save`, so two MyTune processes cannot lose each other's save. A file
    /// rather than a named mutex: it needs no ACL across accounts, and Windows releases it when a process dies.
    fn save_lock(&self) -> std::io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        let mut attempt = 0;
        loop {
            match fs::OpenOptions::new().write(true).create(true).truncate(false).share_mode(0).open(self.dir.join("state.json.lock")) {
                Err(e) if e.raw_os_error() == Some(32) && attempt < 100 => {
                    attempt += 1;
                    sleep(Duration::from_millis(50));
                }
                r => return r,
            }
        }
    }

    pub fn save(&mut self) -> Result<()> {
        self.ensure_read()?;
        let _lock = self.save_lock()?;
        let mine = serde_json::to_value(&self.state)?;
        let mut base = self.base.lock().unwrap_or_else(|e| e.into_inner());
        let path = self.dir.join("state.json");
        // A second MyTune (the comparison copy beside the installed one) may have saved since this process read
        // the file: its changes stay, only what this process changed itself goes over them. Without this the
        // later save dropped the other one's tweak backups.
        let mut out = match read_state(&path) {
            Read::Ok(theirs) => merge_state(&base, mine, serde_json::to_value(&theirs)?),
            Read::Missing => mine,
            Read::Bad => {
                quarantine(&path);
                mine
            }
            Read::Unreadable => return Err(Error::Plain(UNREADABLE.into())),
        };
        // `unmark` leaves `{}` so the merge removes only this process's entries; an empty journal is no key
        if out.get(PENDING).and_then(Value::as_object).is_some_and(Map::is_empty) {
            out.as_object_mut().map(|o| o.remove(PENDING));
        }
        let json = serde_json::to_vec_pretty(&out)?;
        let mut last = None;
        for attempt in 0..3 {
            match self.write_once(&json) {
                Ok(()) => {
                    // memory now matches the file, the other process's backups included
                    if let Ok(s) = serde_json::from_value(out.clone()) {
                        self.state = s;
                    }
                    *base = out;
                    return Ok(());
                }
                Err(e) => {
                    log::error(&format!("state.json save attempt {} failed", attempt + 1), Some(&e));
                    last = Some(e);
                    if attempt < 2 {
                        sleep(Duration::from_millis(100));
                    }
                }
            }
        }
        Err(last.unwrap().into())
    }

    fn write_once(&self, json: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("state.json");
        let tmp = sibling(&path, ".tmp");
        {
            // never reuse a file left here: whoever created it may still hold a write handle
            let _ = fs::remove_file(&tmp);
            let mut f = fs::OpenOptions::new().write(true).create_new(true).share_mode(0).open(&tmp)?;
            f.write_all(json)?;
            f.sync_all()?;
        }
        if path.exists() {
            let w = |p: &Path| -> Vec<u16> { p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect() };
            let (a, b, c) = (w(&path), w(&tmp), w(&sibling(&path, ".bak")));
            unsafe { ReplaceFileW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), PCWSTR(c.as_ptr()), REPLACE_FILE_FLAGS(0), None, None) }
                .map_err(|e| std::io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32))?;
        } else {
            fs::rename(&tmp, &path)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Folder permissions

/// SYSTEM and Administrators full, Users read and run. Protected: %ProgramData% otherwise lets Users create and
/// write here, and state.json tells the elevated app what to write where.
const DIR_ACL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";
const FILE_ACL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;0x1200a9;;;BU)";
/// No read for Users: a read handle opened without sharing would block every save.
const LOCK_ACL: &str = "D:P(A;;FA;;;SY)(A;;FA;;;BA)";
/// Written after a complete pass, so later starts only check the folder itself.
const ACL_DONE: &str = "acl-done";
const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;

fn win_io(e: windows::core::Error) -> std::io::Error {
    std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
}

/// The object itself, never what a link points to.
fn open_meta(p: &Path, access: u32) -> std::io::Result<File> {
    const BACKUP_SEMANTICS_AND_OPEN_REPARSE: u32 = 0x0220_0000;
    fs::OpenOptions::new().access_mode(access).share_mode(7).custom_flags(BACKUP_SEMANTICS_AND_OPEN_REPARSE).open(p)
}

fn set_sddl(f: &File, sddl: &str, info: OBJECT_SECURITY_INFORMATION) -> std::io::Result<()> {
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(text.as_ptr()), SDDL_REVISION_1, &mut sd, None).map_err(win_io)?;
        let r = SetKernelObjectSecurity(HANDLE(f.as_raw_handle()), info, sd);
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        r.map_err(win_io)
    }
}

fn read_sddl(f: &File, info: OBJECT_SECURITY_INFORMATION) -> std::io::Result<String> {
    let handle = HANDLE(f.as_raw_handle());
    let mut needed = 0;
    unsafe {
        let _ = GetKernelObjectSecurity(handle, info.0, None, 0, &mut needed);
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        let sd = PSECURITY_DESCRIPTOR(buf.as_mut_ptr().cast());
        GetKernelObjectSecurity(handle, info.0, Some(sd), needed, &mut needed).map_err(win_io)?;
        let mut text = PWSTR::null();
        ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, info, &mut text, None).map_err(win_io)?;
        let out = text.to_string().unwrap_or_default();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        Ok(out)
    }
}

/// Lets this process take a file whose owner shut Administrators out.
fn take_ownership_privilege(on: bool) {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut token).is_err() {
            return;
        }
        let mut p = TOKEN_PRIVILEGES { PrivilegeCount: 1, ..Default::default() };
        if LookupPrivilegeValueW(None, w!("SeTakeOwnershipPrivilege"), &mut p.Privileges[0].Luid).is_ok() {
            p.Privileges[0].Attributes = if on { SE_PRIVILEGE_ENABLED } else { TOKEN_PRIVILEGES_ATTRIBUTES(0) };
            let _ = AdjustTokenPrivileges(token, false, Some(&p), 0, None, None);
        }
        let _ = CloseHandle(token);
    }
}

/// Chromium's network sandbox capability: WebView2 gave it access to its own profile folders and needs it there.
fn webview_grants(dacl: &str) -> String {
    dacl.split('(')
        .skip(1)
        .filter_map(|ace| {
            let f: Vec<&str> = ace.trim_end_matches(')').split(';').collect();
            (f.len() == 6 && f[0] == "A" && f[5].starts_with("S-1-15-3-1024-")).then(|| format!("(A;{};{};;;{})", f[1].replace("ID", ""), f[2], f[5]))
        })
        .collect()
}

fn secure(p: &Path, acl: &str, webview: bool) -> std::io::Result<()> {
    // two opens: an owner that is not Administrators may have denied them WRITE_DAC until the owner is changed
    set_sddl(&open_meta(p, WRITE_OWNER)?, "O:BA", OWNER_SECURITY_INFORMATION)?;
    let f = open_meta(p, READ_CONTROL | WRITE_DAC)?;
    let now = read_sddl(&f, DACL_SECURITY_INFORMATION)?;
    let want = if webview { format!("{acl}{}", webview_grants(&now)) } else { acl.to_string() };
    if now != want {
        set_sddl(&f, &want, OBJECT_SECURITY_INFORMATION(DACL_SECURITY_INFORMATION.0 | PROTECTED_DACL_SECURITY_INFORMATION.0))?;
    }
    Ok(())
}

fn link_count(p: &Path) -> std::io::Result<u32> {
    let f = open_meta(p, FILE_READ_ATTRIBUTES)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut info) }.map_err(win_io)?;
    Ok(info.nNumberOfLinks)
}

/// Swaps a hard-linked file for its own copy: permissions belong to the file, so setting them would also change
/// whatever the other name is, and a name elsewhere would keep its access to this one.
fn unlink_copy(p: &Path) -> std::io::Result<()> {
    let tmp = sibling(p, ".relink");
    let _ = fs::remove_file(&tmp);
    let data = fs::read(p)?;
    let written = fs::OpenOptions::new().write(true).create_new(true).share_mode(0).open(&tmp).and_then(|mut f| {
        f.write_all(&data)?;
        f.sync_all()
    });
    let done = written.and_then(|()| fs::rename(&tmp, p));
    if done.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    done
}

/// True for a directory to walk into.
fn secure_item(root: &Path, p: &Path) -> std::io::Result<bool> {
    let md = fs::symlink_metadata(p)?;
    // a link planted here would send the next elevated write to its target; removing it leaves the target alone
    if md.file_type().is_symlink() {
        log::info(&format!("folder permissions: removed link {}", p.display()));
        return if md.file_attributes() & 0x10 != 0 { fs::remove_dir(p) } else { fs::remove_file(p) }.map(|()| false);
    }
    if md.is_file() && link_count(p)? > 1 {
        unlink_copy(p)?;
    }
    let acl = if md.is_dir() {
        DIR_ACL
    } else if p == root.join("state.json.lock") {
        LOCK_ACL
    } else {
        FILE_ACL
    };
    secure(p, acl, p.starts_with(root.join("webview")))?;
    Ok(md.is_dir())
}

fn secure_tree(root: &Path, dir: &Path, failed: &mut usize) {
    let Ok(items) = fs::read_dir(dir) else {
        *failed += 1;
        return;
    };
    for p in items.flatten().map(|e| e.path()) {
        match secure_item(root, &p) {
            Ok(true) => secure_tree(root, &p, failed),
            Ok(false) => {}
            Err(e) => {
                *failed += 1;
                log::error(&format!("folder permissions: {}", p.display()), Some(&e));
            }
        }
    }
}

fn try_harden(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(std::io::Error::other("the folder is a link"));
    }
    let done = dir.join(ACL_DONE);
    let info = OBJECT_SECURITY_INFORMATION(OWNER_SECURITY_INFORMATION.0 | DACL_SECURITY_INFORMATION.0);
    if done.exists() && read_sddl(&open_meta(dir, READ_CONTROL)?, info)? == format!("O:BA{DIR_ACL}") {
        return Ok(());
    }
    secure(dir, DIR_ACL, false)?;
    // only now can nobody else put it back
    let _ = fs::remove_file(&done);
    let _ = fs::OpenOptions::new().write(true).create(true).truncate(false).open(dir.join("state.json.lock"));
    let mut failed = 0;
    secure_tree(dir, dir, &mut failed);
    if failed > 0 {
        return Err(std::io::Error::other(format!("{failed} items not changed, tried again at the next start")));
    }
    fs::write(&done, b"")?;
    log::info("folder permissions set: only administrators can change MyTune's files");
    Ok(())
}

/// Closes `dir` to non-admins. The elevated app runs this before it first reads state.json, whose contents it
/// trusts. A failure is logged and never stops the app.
pub fn harden_dir(dir: &Path) {
    take_ownership_privilege(true);
    let r = try_harden(dir);
    take_ownership_privilege(false);
    if let Err(e) = r {
        log::error("folder permissions not fully set", Some(&e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS_JSON: &str = r#"{
  "Profiles": [],
  "Settings": {"Theme": "dark", "Big": 18446744073709551615, "F": 1.5},
  "Applied": {
    "adv.timer": {
      "Title": "t", "AppliedAt": "2026-11-09T23:28:34.9960836+01:00",
      "Registry": [{"Hive":"HKLM","Path":"p","Name":"n","Existed":true,"Kind":"DWord","Value":"1","Multi":null,"CreatedKeys":[]}],
      "Data": {"enabled": "1"}
    }
  },
  "Games": [{"Name": "x"}]
}"#;

    fn bk(title: &str) -> TweakBackup {
        TweakBackup::new(title)
    }

    #[test]
    fn reads_csharp_format_bom_and_keeps_unknown_keys() {
        let d = tempfile::tempdir().unwrap();
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(CS_JSON.as_bytes());
        fs::write(d.path().join("state.json"), raw).unwrap();
        let mut s = Store::load(d.path());
        assert_eq!(s.get_backup("adv.timer").unwrap().registry[0].value.as_deref(), Some("1"));
        s.set_backup("other", bk("o")).unwrap();
        let v: Value = serde_json::from_slice(&fs::read(d.path().join("state.json")).unwrap()).unwrap();
        assert_eq!(v["Settings"]["Big"], serde_json::json!(18446744073709551615u64));
        assert_eq!(v["Settings"]["F"], serde_json::json!(1.5));
        assert_eq!(v["Games"][0]["Name"], "x");
        assert_eq!(v["Applied"]["adv.timer"]["Registry"][0]["Kind"], "DWord");
        assert!(v["Applied"]["other"].is_object());
    }

    /// Two processes on one file: neither save drops what the other wrote, and a removal stays removed.
    #[test]
    fn save_keeps_another_process_changes() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), r#"{"Settings":{"Theme":"dark","GameTimer":false},"Games":[1]}"#).unwrap();
        let (mut a, mut b) = (Store::load(d.path()), Store::load(d.path()));
        a.set_backup("x", bk("from a")).unwrap();
        a.set_settings_flag("GameTimer", true).unwrap();
        b.set_settings_str("AccentColor", "#F2F2F2").unwrap(); // b still holds the file as it was before a's saves
        let s = Store::load(d.path());
        assert_eq!(s.applied_ids(), vec!["x".to_string()]);
        assert_eq!(s.settings_flag("GameTimer"), Some(true));
        assert_eq!(s.settings_str("AccentColor").as_deref(), Some("#F2F2F2"));
        assert_eq!(s.other("Settings").unwrap()["Theme"], "dark");
        b.set_backup("y", bk("from b")).unwrap();
        a.remove_backup("x").unwrap();
        b.set_settings_flag("Other", true).unwrap();
        assert_eq!(Store::load(d.path()).applied_ids(), vec!["y".to_string()]);
    }

    /// The journal key (`Pending`, engine.rs) is a raw top-level key: another process's save keeps it, and
    /// removing it removes it from the file.
    #[test]
    fn pending_key_survives_another_process_save() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), CS_JSON).unwrap();
        let (mut a, mut b) = (Store::load(d.path()), Store::load(d.path()));
        a.set_top("Pending", serde_json::json!({"x": {"Op": "apply"}}));
        a.save().unwrap();
        b.set_settings_flag("GameTimer", true).unwrap(); // b read the file before the entry was there
        let s = Store::load(d.path());
        assert_eq!(s.top("Pending").unwrap()["x"]["Op"], "apply");
        assert_eq!(s.other("Games").unwrap()[0]["Name"], "x");
        a.remove_top("Pending");
        a.save().unwrap();
        b.set_settings_flag("Other", true).unwrap();
        let s = Store::load(d.path());
        assert!(s.top("Pending").is_none());
        assert_eq!(s.settings_flag("GameTimer"), Some(true));
    }

    #[test]
    fn save_keeps_previous_generation_as_bak() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Store::load(d.path());
        s.set_backup("a", bk("first")).unwrap();
        assert!(!d.path().join("state.json.bak").exists());
        s.set_backup("b", bk("second")).unwrap();
        let bak: Value = serde_json::from_slice(&fs::read(d.path().join("state.json.bak")).unwrap()).unwrap();
        assert!(bak["Applied"]["a"].is_object() && bak["Applied"]["b"].is_null());
        assert!(!d.path().join("state.json.tmp").exists());
        s.remove_backup("a").unwrap();
        assert_eq!(Store::load(d.path()).applied_ids(), vec!["b".to_string()]);
    }

    #[test]
    fn settings_flag_roundtrip_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), r#"{"Settings":{"Theme":"dark","GameTimer":false},"Games":[1]}"#).unwrap();
        let mut s = Store::load(d.path());
        assert_eq!(s.settings_flag("GameTimer"), Some(false));
        assert_eq!(s.settings_flag("Nope"), None);
        s.set_settings_flag("GameTimer", true).unwrap();
        let s2 = Store::load(d.path());
        assert_eq!(s2.settings_flag("GameTimer"), Some(true));
        assert_eq!(s2.other("Settings").unwrap()["Theme"], "dark");
        assert_eq!(s2.other("Games").unwrap()[0], 1);
        let empty = tempfile::tempdir().unwrap();
        let mut s3 = Store::load(empty.path());
        s3.set_settings_flag("GameTimer", true).unwrap();
        assert_eq!(Store::load(empty.path()).settings_flag("GameTimer"), Some(true));
    }

    #[test]
    fn null_applied_is_empty() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), r#"{"Applied": null, "X": 1}"#).unwrap();
        let s = Store::load(d.path());
        assert!(s.applied_ids().is_empty());
        assert_eq!(s.other("X"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn falls_back_to_bak_when_state_missing() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json.bak"), CS_JSON).unwrap();
        assert!(Store::load(d.path()).get_backup("adv.timer").is_some());
    }

    #[test]
    fn corrupt_state_is_kept_and_bak_used() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "{ not json").unwrap();
        fs::write(d.path().join("state.json.bak"), CS_JSON).unwrap();
        let mut s = Store::load(d.path());
        assert!(s.get_backup("adv.timer").is_some());
        let corrupt: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("state.json.corrupt-"))
            .collect();
        assert_eq!(corrupt.len(), 1);
        assert_eq!(fs::read_to_string(corrupt[0].path()).unwrap(), "{ not json");
        // next save must not rotate the corrupt file over the good .bak
        s.set_backup("n", bk("n")).unwrap();
        assert!(fs::read_to_string(d.path().join("state.json")).unwrap().contains("adv.timer"));
    }

    /// A directory named state.json: reading it is an I/O error, like a locked file, without the 3 s of retries.
    #[test]
    fn unreadable_state_is_not_moved_replaced_or_overwritten() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("state.json");
        fs::create_dir(&path).unwrap();
        fs::write(d.path().join("state.json.bak"), r#"{"Settings":{"Old":true}}"#).unwrap();
        let mut s = Store::load(d.path());
        assert!(path.is_dir(), "not moved aside");
        assert_eq!(s.settings_flag("Old"), None, "the older .bak is not loaded in its place");
        assert!(s.set_settings_flag("Mine", true).is_err(), "no save while it cannot be read");
        assert!(path.is_dir());
        assert!(fs::read_to_string(d.path().join("state.json.bak")).unwrap().contains("Old"));
        // readable again: its content comes in, and the change made meanwhile stays
        fs::remove_dir(&path).unwrap();
        fs::write(&path, CS_JSON).unwrap();
        s.save().unwrap();
        assert!(s.get_backup("adv.timer").is_some());
        let s2 = Store::load(d.path());
        assert!(s2.get_backup("adv.timer").is_some());
        assert_eq!(s2.settings_flag("Mine"), Some(true));
        assert_eq!(s2.other("Settings").unwrap()["Theme"], "dark");
    }

    #[test]
    fn two_stores_saving_at_once_lose_nothing() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "{}").unwrap();
        let handles: Vec<_> = (0..4)
            .map(|n| {
                let dir = d.path().to_path_buf();
                std::thread::spawn(move || {
                    let mut s = Store::load(&dir);
                    for i in 0..10 {
                        s.set_backup(&format!("t{n}-{i}"), bk("x")).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(Store::load(d.path()).applied_ids().len(), 40);
    }

    #[test]
    fn failed_save_keeps_backup_in_memory_on_remove() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Store::load(d.path());
        s.set_backup("t", bk("t")).unwrap();
        fs::create_dir(d.path().join("state.json.tmp")).unwrap(); // File::create on it fails
        assert!(s.remove_backup("t").is_err());
        assert!(s.get_backup("t").is_some());
    }

    #[test]
    fn null_fields_keep_every_backup() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("state.json"),
            r#"{"Applied":{"a":{"Title":null,"Registry":null,"Data":null},"b":{"Registry":[{"Hive":"HKLM","Path":"p","Name":"n","Existed":false,"CreatedKeys":null}],"Data":{"k":"v"}}}}"#,
        )
        .unwrap();
        let s = Store::load(d.path());
        assert!(s.get_backup("a").unwrap().registry.is_empty());
        assert!(s.get_backup("b").unwrap().registry[0].created_keys.is_empty());
    }

    #[test]
    fn bad_state_and_bad_bak_block_saves() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "{ not json").unwrap();
        fs::write(d.path().join("state.json.bak"), "{ also bad").unwrap();
        let mut s = Store::load(d.path());
        assert!(s.set_settings_flag("X", true).is_err());
        assert!(!d.path().join("state.json").exists());
        assert_eq!(fs::read_to_string(d.path().join("state.json.bak")).unwrap(), "{ also bad");
        assert!(Store::load(d.path()).set_settings_flag("X", true).is_err(), "still blocked after a restart");
    }

    #[test]
    fn backup_both_sides_created_stays_as_on_disk() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, mut b) = (Store::load(d.path()), Store::load(d.path()));
        let with = |v: &str| {
            let mut x = bk("x");
            x.data.insert("v".into(), v.into());
            x
        };
        a.set_backup("x", with("true original")).unwrap();
        b.set_backup("x", with("already tweaked")).unwrap();
        assert_eq!(Store::load(d.path()).get_backup("x").unwrap().data["v"], "true original");
        assert_eq!(b.get_backup("x").unwrap().data["v"], "true original", "memory follows the file");
    }

    #[test]
    fn one_process_clearing_its_journal_keeps_the_others() {
        let d = tempfile::tempdir().unwrap();
        let (mut a, mut b) = (Store::load(d.path()), Store::load(d.path()));
        a.set_top(PENDING, serde_json::json!({"x": {"Op": "apply"}}));
        a.save().unwrap();
        b.set_top(PENDING, serde_json::json!({"y": {"Op": "apply"}}));
        b.save().unwrap();
        a.set_top(PENDING, serde_json::json!({})); // as `unmark` leaves it
        a.save().unwrap();
        let s = Store::load(d.path());
        assert!(s.top(PENDING).unwrap().get("x").is_none());
        assert_eq!(s.top(PENDING).unwrap()["y"]["Op"], "apply");
        b.set_top(PENDING, serde_json::json!({}));
        b.save().unwrap();
        assert!(Store::load(d.path()).top(PENDING).is_none(), "an empty journal is no key");
    }

    #[test]
    fn a_planted_tmp_file_is_not_reused() {
        let d = tempfile::tempdir().unwrap();
        let mut held = File::create(d.path().join("state.json.tmp")).unwrap();
        let mut s = Store::load(d.path());
        s.set_backup("a", bk("a")).unwrap();
        held.write_all(b"planted").unwrap();
        assert!(Store::load(d.path()).get_backup("a").is_some());
    }

    /// Only ever on a scratch folder. Setting the owner needs an elevated test run.
    #[test]
    fn harden_dir_closes_the_folder_and_keeps_webview_grants() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("MyTune");
        let net = dir.join("webview").join("Network");
        fs::create_dir_all(&net).unwrap();
        if open_meta(&dir, WRITE_OWNER).and_then(|f| set_sddl(&f, "O:BA", OWNER_SECURITY_INFORMATION)).is_err() {
            eprintln!("skipped: needs elevation");
            return;
        }
        fs::write(dir.join("state.json"), "{}").unwrap();
        fs::write(net.join("Cookies"), "c").unwrap();
        fs::write(dir.join("linked"), "keep").unwrap();
        fs::hard_link(dir.join("linked"), d.path().join("other-name")).unwrap();
        let cap = "S-1-15-3-1024-1-2-3-4-5-6-7-8";
        let dacl = format!("D:(A;OICI;FA;;;BA)(A;OICI;FA;;;BU)(A;OICI;0x1301bf;;;{cap})");
        set_sddl(&open_meta(&net, READ_CONTROL | WRITE_DAC).unwrap(), &dacl, DACL_SECURITY_INFORMATION).unwrap();
        let target = d.path().join("target");
        fs::write(&target, "t").unwrap();
        let before = read_sddl(&open_meta(&target, READ_CONTROL).unwrap(), DACL_SECURITY_INFORMATION).unwrap();
        let linked = std::os::windows::fs::symlink_file(&target, dir.join("planted")).is_ok();

        harden_dir(&dir);

        let full = OBJECT_SECURITY_INFORMATION(OWNER_SECURITY_INFORMATION.0 | DACL_SECURITY_INFORMATION.0);
        let sd = |p: &Path| read_sddl(&open_meta(p, READ_CONTROL).unwrap(), full).unwrap();
        assert_eq!(sd(&dir), format!("O:BA{DIR_ACL}"));
        assert_eq!(sd(&dir.join("state.json")), format!("O:BA{FILE_ACL}"));
        assert_eq!(sd(&dir.join("state.json.lock")), format!("O:BA{LOCK_ACL}"));
        assert_eq!(sd(&net), format!("O:BA{DIR_ACL}(A;OICI;0x1301bf;;;{cap})"), "its grant stays, Users write goes");
        assert!(dir.join(ACL_DONE).exists());
        assert_eq!(link_count(&dir.join("linked")).unwrap(), 1);
        assert_eq!(fs::read_to_string(dir.join("linked")).unwrap(), "keep");
        if linked {
            assert!(fs::symlink_metadata(dir.join("planted")).is_err(), "the link is gone");
            assert!(target.exists());
            assert_eq!(read_sddl(&open_meta(&target, READ_CONTROL).unwrap(), DACL_SECURITY_INFORMATION).unwrap(), before, "its target was not changed");
        }
    }

    #[test]
    fn empty_file_is_corrupt_and_no_bak_gives_empty_state() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "").unwrap();
        let s = Store::load(d.path());
        assert!(s.applied_ids().is_empty());
        assert!(!d.path().join("state.json").exists());
        assert!(fs::read_dir(d.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".corrupt-")));
    }
}
