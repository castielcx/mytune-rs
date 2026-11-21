//! Registry helpers. Always the 64-bit view (like C# `Reg`, `RegistryView.Registry64`).

use crate::error::{Error, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    Hklm,
    Hkcu,
    Hku,
}

impl Hive {
    pub fn name(self) -> &'static str {
        match self {
            Hive::Hklm => "HKLM",
            Hive::Hkcu => "HKCU",
            Hive::Hku => "HKU",
        }
    }
    pub fn parse(s: &str) -> Option<Hive> {
        match s.to_ascii_uppercase().as_str() {
            "HKLM" => Some(Hive::Hklm),
            "HKCU" => Some(Hive::Hkcu),
            "HKU" => Some(Hive::Hku),
            _ => None,
        }
    }
    fn root(self) -> HKEY {
        match self {
            Hive::Hklm => HKEY_LOCAL_MACHINE,
            Hive::Hkcu => HKEY_CURRENT_USER,
            Hive::Hku => HKEY_USERS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    Dword(u32),
    Qword(u64),
    String(String),
    ExpandString(String),
    MultiString(Vec<String>),
    Binary(Vec<u8>),
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn err(op: &'static str, path: &str, code: WIN32_ERROR) -> Error {
    Error::Registry { op, path: path.to_string(), code: code.0 }
}

fn not_found(code: WIN32_ERROR) -> bool {
    code == ERROR_FILE_NOT_FOUND || code == ERROR_PATH_NOT_FOUND
}

/// Why a tweak that writes the user's settings is refused when MyTune runs under another account.
pub const OTHER_ACCOUNT: &str = "MyTune is running as a different Windows account than the one signed in, so this would change the wrong account's settings. Nothing was changed. Sign in with an administrator account and start MyTune there.";

thread_local! {
    static HKCU_DENIED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// While this lives, HKCU writes on this thread fail with `OTHER_ACCOUNT`.
pub(crate) struct DenyHkcu;

pub(crate) fn deny_hkcu_writes() -> DenyHkcu {
    HKCU_DENIED.set(true);
    DenyHkcu
}

impl Drop for DenyHkcu {
    fn drop(&mut self) {
        HKCU_DENIED.set(false);
    }
}

fn writable(hive: Hive) -> Result<()> {
    if hive == Hive::Hkcu && HKCU_DENIED.get() {
        return Err(Error::Plain(OTHER_ACCOUNT.into()));
    }
    Ok(())
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// Opens a key; `Ok(None)` when it does not exist.
fn open(hive: Hive, path: &str, access: REG_SAM_FLAGS) -> Result<Option<Key>> {
    let p = wide(path);
    let mut h = HKEY::default();
    let r = unsafe { RegOpenKeyExW(hive.root(), PCWSTR(p.as_ptr()), None, access | KEY_WOW64_64KEY, &mut h) };
    if r == ERROR_SUCCESS {
        Ok(Some(Key(h)))
    } else if not_found(r) {
        Ok(None)
    } else {
        Err(err("open", path, r))
    }
}

fn create(hive: Hive, path: &str, access: REG_SAM_FLAGS) -> Result<Key> {
    writable(hive)?;
    let p = wide(path);
    let mut h = HKEY::default();
    let r = unsafe {
        RegCreateKeyExW(
            hive.root(),
            PCWSTR(p.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            access | KEY_WOW64_64KEY,
            None,
            &mut h,
            None,
        )
    };
    if r == ERROR_SUCCESS {
        Ok(Key(h))
    } else {
        Err(err("create", path, r))
    }
}

pub fn key_exists(hive: Hive, path: &str) -> Result<bool> {
    Ok(open(hive, path, KEY_READ)?.is_some())
}

pub fn create_key(hive: Hive, path: &str) -> Result<()> {
    create(hive, path, KEY_READ).map(|_| ())
}

/// Prefixes of `path` that do not exist yet, shallowest first (what a later write would create).
pub fn missing_keys(hive: Hive, path: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for part in path.split('\\').filter(|s| !s.is_empty()) {
        if !cur.is_empty() {
            cur.push('\\');
        }
        cur.push_str(part);
        if !key_exists(hive, &cur)? {
            out.push(cur.clone());
        }
    }
    Ok(out)
}

fn decode(ty: u32, data: &[u8]) -> Result<RegValue> {
    let utf16 = |d: &[u8]| -> Vec<u16> { d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
    Ok(match REG_VALUE_TYPE(ty) {
        REG_DWORD if data.len() == 4 => RegValue::Dword(u32::from_le_bytes(data.try_into().unwrap())),
        REG_QWORD if data.len() == 8 => RegValue::Qword(u64::from_le_bytes(data.try_into().unwrap())),
        REG_SZ | REG_EXPAND_SZ => {
            let mut w = utf16(data);
            while w.last() == Some(&0) {
                w.pop();
            }
            let s = String::from_utf16_lossy(&w);
            if REG_VALUE_TYPE(ty) == REG_SZ {
                RegValue::String(s)
            } else {
                RegValue::ExpandString(s)
            }
        }
        REG_MULTI_SZ => {
            let mut w = utf16(data);
            while w.last() == Some(&0) {
                w.pop();
            }
            if w.is_empty() {
                RegValue::MultiString(vec![])
            } else {
                RegValue::MultiString(w.split(|&c| c == 0).map(String::from_utf16_lossy).collect())
            }
        }
        REG_BINARY => RegValue::Binary(data.to_vec()),
        _ => return Err(Error::UnsupportedKind(ty)),
    })
}

fn encode(v: &RegValue) -> (REG_VALUE_TYPE, Vec<u8>) {
    let utf16z = |s: &str| -> Vec<u8> { wide(s).iter().flat_map(|c| c.to_le_bytes()).collect() };
    match v {
        RegValue::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
        RegValue::Qword(q) => (REG_QWORD, q.to_le_bytes().to_vec()),
        RegValue::String(s) => (REG_SZ, utf16z(s)),
        RegValue::ExpandString(s) => (REG_EXPAND_SZ, utf16z(s)),
        RegValue::MultiString(list) => {
            let mut b: Vec<u8> = list.iter().flat_map(|s| utf16z(s)).collect();
            b.extend_from_slice(&[0, 0]);
            (REG_MULTI_SZ, b)
        }
        RegValue::Binary(b) => (REG_BINARY, b.clone()),
    }
}

/// Reads a value; `Ok(None)` when the key or value is missing.
pub fn get(hive: Hive, path: &str, name: &str) -> Result<Option<RegValue>> {
    let Some(key) = open(hive, path, KEY_QUERY_VALUE)? else { return Ok(None) };
    let n = wide(name);
    let mut buf = vec![0u8; 256];
    loop {
        let mut ty = REG_VALUE_TYPE(0);
        let mut size = buf.len() as u32;
        let r = unsafe { RegQueryValueExW(key.0, PCWSTR(n.as_ptr()), None, Some(&mut ty), Some(buf.as_mut_ptr()), Some(&mut size)) };
        if r == ERROR_SUCCESS {
            return decode(ty.0, &buf[..size as usize]).map(Some);
        } else if r == ERROR_MORE_DATA {
            buf.resize((size as usize).max(buf.len() * 2), 0);
        } else if not_found(r) {
            return Ok(None);
        } else {
            return Err(err("read", &format!("{path}\\{name}"), r));
        }
    }
}

/// Names of the values of a key (empty when the key is missing).
pub fn value_names(hive: Hive, path: &str) -> Result<Vec<String>> {
    let Some(key) = open(hive, path, KEY_QUERY_VALUE)? else { return Ok(vec![]) };
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = vec![0u16; 16384]; // registry value names are at most 16383 characters
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumValueW(key.0, i, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if r == ERROR_SUCCESS {
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        } else if r == windows::Win32::Foundation::ERROR_NO_MORE_ITEMS {
            break;
        } else {
            return Err(err("enumerate", path, r));
        }
    }
    Ok(out)
}

/// Names of the keys directly under a key (empty when the key is missing).
pub fn subkeys(hive: Hive, path: &str) -> Result<Vec<String>> {
    let Some(key) = open(hive, path, KEY_ENUMERATE_SUB_KEYS)? else { return Ok(vec![]) };
    let mut out = Vec::new();
    for i in 0.. {
        let mut buf = [0u16; 256]; // registry key names are at most 255 characters
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumKeyExW(key.0, i, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if r == ERROR_SUCCESS {
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        } else if r == windows::Win32::Foundation::ERROR_NO_MORE_ITEMS {
            break;
        } else {
            return Err(err("enumerate", path, r));
        }
    }
    Ok(out)
}

/// Writes a value, creating the key (and parents) when needed.
pub fn set(hive: Hive, path: &str, name: &str, value: &RegValue) -> Result<()> {
    let key = create(hive, path, KEY_SET_VALUE)?;
    let n = wide(name);
    let (ty, data) = encode(value);
    let r = unsafe { RegSetValueExW(key.0, PCWSTR(n.as_ptr()), None, ty, Some(&data)) };
    if r == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(err("write", &format!("{path}\\{name}"), r))
    }
}

/// Deletes a value; missing key or value is fine.
pub fn delete_value(hive: Hive, path: &str, name: &str) -> Result<()> {
    writable(hive)?;
    let Some(key) = open(hive, path, KEY_SET_VALUE)? else { return Ok(()) };
    let n = wide(name);
    let r = unsafe { RegDeleteValueW(key.0, PCWSTR(n.as_ptr())) };
    if r == ERROR_SUCCESS || not_found(r) {
        Ok(())
    } else {
        Err(err("delete value", &format!("{path}\\{name}"), r))
    }
}

/// Deletes the key only when it has no subkeys and no values. Returns whether it was deleted.
pub fn delete_key_if_empty(hive: Hive, path: &str) -> Result<bool> {
    writable(hive)?;
    let Some(key) = open(hive, path, KEY_READ)? else { return Ok(false) };
    let (mut subkeys, mut values) = (0u32, 0u32);
    let r = unsafe {
        RegQueryInfoKeyW(key.0, None, None, None, Some(&mut subkeys), None, None, Some(&mut values), None, None, None, None)
    };
    if r != ERROR_SUCCESS {
        return Err(err("query", path, r));
    }
    drop(key);
    if subkeys != 0 || values != 0 {
        return Ok(false);
    }
    let p = wide(path);
    let r = unsafe { RegDeleteKeyExW(hive.root(), PCWSTR(p.as_ptr()), KEY_WOW64_64KEY.0, None) };
    if r == ERROR_SUCCESS {
        Ok(true)
    } else if not_found(r) {
        Ok(false)
    } else {
        Err(err("delete key", path, r))
    }
}

/// Deletes a key and everything below it; missing key is fine.
pub fn delete_tree(hive: Hive, path: &str) -> Result<()> {
    writable(hive)?;
    let Some(key) = open(hive, path, KEY_ALL_ACCESS)? else { return Ok(()) };
    let r = unsafe { RegDeleteTreeW(key.0, PCWSTR::null()) };
    drop(key);
    if r != ERROR_SUCCESS && !not_found(r) {
        return Err(err("delete tree", path, r));
    }
    let p = wide(path);
    let r = unsafe { RegDeleteKeyExW(hive.root(), PCWSTR(p.as_ptr()), KEY_WOW64_64KEY.0, None) };
    if r == ERROR_SUCCESS || not_found(r) {
        Ok(())
    } else {
        Err(err("delete key", path, r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Throwaway key; removed on drop. Unique per test so parallel tests do not collide.
    struct Scratch(String);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = format!("Software\\MyTuneTest\\{tag}-{}", std::process::id());
            let _ = delete_tree(Hive::Hkcu, &p);
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete_tree(Hive::Hkcu, &self.0);
            let _ = delete_key_if_empty(Hive::Hkcu, "Software\\MyTuneTest");
        }
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn roundtrip_all_types() {
        let s = Scratch::new("types");
        let vals = [
            ("d", RegValue::Dword(0xFFFF_FFFF)),
            ("q", RegValue::Qword(u64::MAX)),
            ("s", RegValue::String("héllo".into())),
            ("e", RegValue::ExpandString("%PATH%\\x".into())),
            ("m", RegValue::MultiString(vec!["a".into(), "b c".into()])),
            ("m0", RegValue::MultiString(vec![])),
            ("b", RegValue::Binary(vec![0, 1, 2, 255])),
            ("b0", RegValue::Binary(vec![])),
        ];
        for (n, v) in &vals {
            set(Hive::Hkcu, &s.0, n, v).unwrap();
        }
        for (n, v) in &vals {
            assert_eq!(get(Hive::Hkcu, &s.0, n).unwrap().as_ref(), Some(v), "{n}");
        }
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn long_value_grows_buffer() {
        let s = Scratch::new("long");
        let v = RegValue::String("x".repeat(5000));
        set(Hive::Hkcu, &s.0, "v", &v).unwrap();
        assert_eq!(get(Hive::Hkcu, &s.0, "v").unwrap(), Some(v));
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn missing_and_delete() {
        let s = Scratch::new("del");
        assert_eq!(get(Hive::Hkcu, &s.0, "x").unwrap(), None);
        assert!(!key_exists(Hive::Hkcu, &s.0).unwrap());
        set(Hive::Hkcu, &s.0, "x", &RegValue::Dword(1)).unwrap();
        assert_eq!(get(Hive::Hkcu, &s.0, "nope").unwrap(), None);
        delete_value(Hive::Hkcu, &s.0, "x").unwrap();
        delete_value(Hive::Hkcu, &s.0, "x").unwrap(); // already gone
        assert_eq!(get(Hive::Hkcu, &s.0, "x").unwrap(), None);
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn missing_keys_and_delete_if_empty() {
        let s = Scratch::new("keys");
        let deep = format!("{}\\a\\b", s.0);
        let missing = missing_keys(Hive::Hkcu, &deep).unwrap();
        assert_eq!(missing, vec![s.0.clone(), format!("{}\\a", s.0), deep.clone()]);
        set(Hive::Hkcu, &deep, "v", &RegValue::Dword(1)).unwrap();
        assert!(!delete_key_if_empty(Hive::Hkcu, &deep).unwrap(), "has a value");
        delete_value(Hive::Hkcu, &deep, "v").unwrap();
        assert!(delete_key_if_empty(Hive::Hkcu, &deep).unwrap());
        assert!(!key_exists(Hive::Hkcu, &deep).unwrap());
        assert!(!delete_key_if_empty(Hive::Hkcu, &deep).unwrap(), "already gone");
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn delete_tree_removes_subkeys() {
        let s = Scratch::new("tree");
        set(Hive::Hkcu, &format!("{}\\a\\b", s.0), "v", &RegValue::Dword(1)).unwrap();
        delete_tree(Hive::Hkcu, &s.0).unwrap();
        assert!(!key_exists(Hive::Hkcu, &s.0).unwrap());
        delete_tree(Hive::Hkcu, &s.0).unwrap(); // missing is fine
    }

    /// HKLM needs admin: skips (passes) with a note when access is denied.
    #[test]
    #[ignore = "real system: writes HKLM scratch key"]
    fn hklm_roundtrip_in_64bit_view() {
        let path = format!("SOFTWARE\\MyTuneTest\\hklm-{}", std::process::id());
        let _ = delete_tree(Hive::Hklm, &path);
        match set(Hive::Hklm, &path, "D", &RegValue::Dword(42)) {
            Err(Error::Registry { code: 5, .. }) => {
                eprintln!("SKIPPED: not elevated, HKLM untested");
                return;
            }
            r => r.unwrap(),
        }
        set(Hive::Hklm, &path, "M", &RegValue::MultiString(vec!["x".into(), "y".into()])).unwrap();
        assert_eq!(get(Hive::Hklm, &path, "D").unwrap(), Some(RegValue::Dword(42)));
        assert_eq!(get(Hive::Hklm, &path, "M").unwrap(), Some(RegValue::MultiString(vec!["x".into(), "y".into()])));
        // independent check through reg.exe in the 64-bit view
        let q = crate::process::run("reg", &["query", &format!("HKLM\\{path}"), "/v", "D", "/reg:64"]).unwrap();
        assert!(q.output.contains("0x2a"), "{}", q.output);
        delete_tree(Hive::Hklm, &path).unwrap();
        assert!(!key_exists(Hive::Hklm, &path).unwrap());
        let _ = delete_key_if_empty(Hive::Hklm, "SOFTWARE\\MyTuneTest");
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn default_value_name() {
        let s = Scratch::new("def");
        set(Hive::Hkcu, &s.0, "", &RegValue::String("dflt".into())).unwrap();
        assert_eq!(get(Hive::Hkcu, &s.0, "").unwrap(), Some(RegValue::String("dflt".into())));
    }
}
