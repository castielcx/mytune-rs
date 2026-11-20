//! Licence key check for the public setup (Cargo feature `license`; the owner's own build has no check).
//! A key is bound to this PC by the key server, which answers with a signed token good for 7 days.
//! The token is kept in `%ProgramData%\MyTune\license.json`, so MyTune works that long without the server.

use crate::reg::{self, Hive, RegValue};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Mutex, Once, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// The owner fills in both after deploying the key server. While either is empty the check is off
// and the app is unlocked.
/// Address of the key server without a trailing slash, e.g. `https://keys.example.com`.
pub const SERVER_URL: &str = "https://mytune-keys.mytune.workers.dev";
/// The server's Ed25519 public key: 32 raw bytes as standard base64.
pub const PUBLIC_KEY: &str = "9DS8EdE9dMia10w/5QoACFCo+iUoxz1xLxosENlxs3w=";

/// Why the app is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// The server knows no such key.
    Invalid,
    /// The owner switched the key off.
    Revoked,
    /// The key is bound to another PC.
    Bound,
    /// The server gave no usable answer.
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Checking,
    Unlocked,
    /// `None`: no key was entered on this PC yet.
    Locked(Option<Reject>),
}

static STATUS: Mutex<Status> = Mutex::new(Status::Checking);

fn set(s: Status) {
    *STATUS.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

/// Cheap enough for every frame. The first call starts the check and the hourly recheck thread.
pub fn status() -> Status {
    static START: Once = Once::new();
    START.call_once(|| {
        if SERVER_URL.is_empty() || PUBLIC_KEY.is_empty() {
            return set(Status::Unlocked);
        }
        std::thread::spawn(|| {
            device_id(); // before any Locked status: the gate shows it
            for hour in 0u64.. {
                check(hour % 24 == 0);
                std::thread::sleep(Duration::from_secs(3600));
            }
        });
    });
    *STATUS.lock().unwrap_or_else(|e| e.into_inner())
}

/// One round of the background check. The server is asked every 24 h, and every hour while the token is no
/// longer good (so the app unlocks by itself once the server is reachable again).
fn check(online: bool) {
    let Some(c) = load() else {
        if status() == Status::Checking {
            set(Status::Locked(None));
        }
        return;
    };
    let good = verify(&c.token, device_id(), now()).is_ok();
    if good && status() == Status::Checking {
        set(Status::Unlocked); // a good cached token: no wait for the server
    }
    if online || !good {
        if let Err(Reject::Network) = activate(&c.key) {
            if !good {
                set(Status::Locked(Some(Reject::Network)));
            }
        }
    }
}

/// The gate's Activate button: shows Checking, asks the server on a thread, then Unlocked or Locked.
pub fn submit(key: String) {
    set(Status::Checking);
    std::thread::spawn(move || {
        if let Err(Reject::Network) = activate(&key) {
            set(Status::Locked(Some(Reject::Network)));
        }
    });
}

/// Asks the server to bind `key` to this PC (first activation and every recheck alike). A token unlocks and
/// is saved; a refusal locks (Invalid and Revoked also delete the saved key); a network error changes nothing.
pub fn activate(key: &str) -> Result<(), Reject> {
    let key = key.trim().to_uppercase();
    let body = serde_json::json!({ "key": key, "device": device_id(), "name": std::env::var("COMPUTERNAME").unwrap_or_default() });
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let answer = agent.post(&format!("{}/activate", SERVER_URL.trim_end_matches('/'))).set("Content-Type", "application/json").send_string(&body.to_string());
    let field = |r: ureq::Response, name: &str| {
        let v: Option<serde_json::Value> = r.into_string().ok().and_then(|s| serde_json::from_str(&s).ok());
        v.and_then(|v| v[name].as_str().map(str::to_owned))
    };
    match answer {
        Ok(r) => {
            let token = field(r, "token").ok_or(Reject::Network)?;
            // a token that does not verify is a broken or fake server, not a refusal of the key
            verify(&token, device_id(), now()).map_err(|_| Reject::Network)?;
            let _ = std::fs::create_dir_all(crate::util::default_dir());
            let _ = std::fs::write(file(), serde_json::to_string(&Cached { key, token }).unwrap_or_default());
            set(Status::Unlocked);
            Ok(())
        }
        Err(ureq::Error::Status(403, r)) => {
            let why = refusal(field(r, "error").as_deref());
            // Bound may be a device id that changes back (adapter not up yet at logon): keep the key for the hourly retry
            if matches!(why, Reject::Invalid | Reject::Revoked) {
                let _ = std::fs::remove_file(file());
            }
            if why != Reject::Network {
                set(Status::Locked(Some(why)));
            }
            Err(why)
        }
        Err(_) => Err(Reject::Network),
    }
}

/// The reason in a 403 answer. Anything but the server's own words (a captive portal, a proxy or a
/// block page) is a network error, so it never deletes the saved key.
fn refusal(error: Option<&str>) -> Reject {
    match error {
        Some("invalid") => Reject::Invalid,
        Some("revoked") => Reject::Revoked,
        Some("bound") => Reject::Bound,
        _ => Reject::Network,
    }
}

/// The key saved on this PC, if any (the gate retries it when the field is left empty).
pub fn cached_key() -> Option<String> {
    load().map(|c| c.key)
}

#[derive(Serialize, Deserialize)]
struct Cached {
    key: String,
    token: String,
}

fn file() -> PathBuf {
    crate::util::default_dir().join("license.json")
}

fn load() -> Option<Cached> {
    serde_json::from_str(&std::fs::read_to_string(file()).ok()?).ok()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Deserialize)]
struct Payload {
    device: String,
    exp: u64,
}

/// A token is good when the server signed it, it names this PC and it has not run out.
fn verify(token: &str, device: &str, now: u64) -> Result<(), ()> {
    let raw: [u8; 32] = STANDARD.decode(PUBLIC_KEY).ok().and_then(|b| b.try_into().ok()).ok_or(())?;
    verify_with(&VerifyingKey::from_bytes(&raw).map_err(drop)?, token, device, now)
}

fn verify_with(public: &VerifyingKey, token: &str, device: &str, now: u64) -> Result<(), ()> {
    let (payload, sig) = token.split_once('.').ok_or(())?;
    let payload = URL_SAFE_NO_PAD.decode(payload).map_err(drop)?;
    let sig = Signature::from_slice(&URL_SAFE_NO_PAD.decode(sig).map_err(drop)?).map_err(drop)?;
    public.verify_strict(&payload, &sig).map_err(drop)?;
    let p: Payload = serde_json::from_slice(&payload).map_err(drop)?;
    if p.device == device && p.exp > now { Ok(()) } else { Err(()) }
}

// ---------------------------------------------------------------- this PC

/// SHA-256 (64 hex characters) of the Windows MachineGuid, the motherboard serial and the built-in MAC of
/// the first network adapter on the PCI bus. Only this hash leaves the PC. A part that cannot be read is
/// empty. Worked out once; the first call takes a few milliseconds, so it is made off the UI thread.
pub fn device_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let guid = match reg::get(Hive::Hklm, r"SOFTWARE\Microsoft\Cryptography", "MachineGuid") {
            Ok(Some(RegValue::String(s))) => s,
            _ => String::new(),
        };
        let text = format!("{guid}|{}|{}", board_serial(&smbios()), first_mac());
        Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    })
}

/// Windows' raw copy of the firmware's SMBIOS tables.
fn smbios() -> Vec<u8> {
    use windows::Win32::System::SystemInformation::{GetSystemFirmwareTable, FIRMWARE_TABLE_PROVIDER};
    let rsmb = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
    unsafe {
        let mut buf = vec![0u8; GetSystemFirmwareTable(rsmb, 0, None) as usize];
        let n = GetSystemFirmwareTable(rsmb, 0, Some(&mut buf)) as usize;
        buf.truncate(n.min(buf.len()));
        buf
    }
}

/// The baseboard (type 2) serial number from raw SMBIOS data, empty when there is none.
fn board_serial(raw: &[u8]) -> String {
    let mut t = raw.get(8..).unwrap_or_default(); // 8 bytes of Windows' own header
    while t.len() >= 4 && t[1] >= 4 && t.len() >= t[1] as usize {
        let (kind, len) = (t[0], t[1] as usize);
        // after the fixed part: strings, each closed by a zero, the list closed by one more zero
        let end = t[len..].windows(2).position(|w| w == [0, 0]).map_or(t.len(), |i| len + i + 2);
        if kind == 2 && len > 7 && t[7] > 0 {
            let s = t[len..end].split(|&b| b == 0).nth(t[7] as usize - 1).unwrap_or_default();
            return String::from_utf8_lossy(s).trim().to_owned();
        }
        t = &t[end.min(t.len())..];
    }
    String::new()
}

/// The lowest built-in ("permanent") MAC among the network adapters on the PCI bus. VPN and other virtual
/// adapters and USB dongles are not on that bus, and a changed or random MAC does not change the built-in one.
fn first_mac() -> String {
    const NET: &str = r"SYSTEM\CurrentControlSet\Control\Network\{4D36E972-E325-11CE-BFC1-08002BE10318}";
    let mut macs: Vec<String> = reg::subkeys(Hive::Hklm, NET)
        .unwrap_or_default()
        .into_iter()
        .filter(|g| matches!(reg::get(Hive::Hklm, &format!(r"{NET}\{g}\Connection"), "PnpInstanceId"), Ok(Some(RegValue::String(s))) if s.to_uppercase().starts_with(r"PCI\")))
        .filter_map(|g| permanent_mac(&g))
        .collect();
    macs.sort();
    macs.into_iter().next().unwrap_or_default()
}

/// Asks the adapter's driver for the MAC it was built with (OID_802_3_PERMANENT_ADDRESS).
fn permanent_mac(guid: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::IO::DeviceIoControl;
    const IOCTL_NDIS_QUERY_GLOBAL_STATS: u32 = 0x0017_0002;
    let path: Vec<u16> = format!(r"\\.\{guid}").encode_utf16().chain([0]).collect();
    let oid = 0x0101_0101u32;
    let (mut mac, mut got) = ([0u8; 6], 0u32);
    unsafe {
        let h = CreateFileW(PCWSTR(path.as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).ok()?;
        let ok = DeviceIoControl(h, IOCTL_NDIS_QUERY_GLOBAL_STATS, Some(&oid as *const u32 as _), 4, Some(mac.as_mut_ptr() as _), 6, Some(&mut got), None);
        let _ = CloseHandle(h);
        ok.ok()?;
    }
    (got == 6 && mac != [0; 6]).then(|| mac.iter().map(|b| format!("{b:02X}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const DEVICE: &str = "aa11";

    fn token(sk: &SigningKey, payload: &str) -> String {
        format!("{}.{}", URL_SAFE_NO_PAD.encode(payload), URL_SAFE_NO_PAD.encode(sk.sign(payload.as_bytes()).to_bytes()))
    }

    #[test]
    fn token_checks() {
        let sk = SigningKey::from_bytes(&[7; 32]);
        let pk = sk.verifying_key();
        let good = token(&sk, r#"{"key":"AAAAA-BBBBB-CCCCC-DDDDD","device":"aa11","exp":2000}"#);
        assert!(verify_with(&pk, &good, DEVICE, 1000).is_ok());
        assert!(verify_with(&pk, &good, "bb22", 1000).is_err(), "another PC");
        assert!(verify_with(&pk, &good, DEVICE, 2000).is_err(), "run out");
        // a payload changed after signing: a later exp under the old signature
        let forged = format!("{}.{}", URL_SAFE_NO_PAD.encode(r#"{"key":"AAAAA-BBBBB-CCCCC-DDDDD","device":"aa11","exp":9000}"#), good.split_once('.').unwrap().1);
        assert!(verify_with(&pk, &forged, DEVICE, 1000).is_err(), "changed payload");
        assert!(verify_with(&SigningKey::from_bytes(&[8; 32]).verifying_key(), &good, DEVICE, 1000).is_err(), "another server key");
        assert!(verify_with(&pk, "nonsense", DEVICE, 1000).is_err());
    }

    /// A token the live server gave out (env MYTUNE_TEST_TOKEN, for device MYTUNE_TEST_DEVICE) passes `PUBLIC_KEY`.
    #[test]
    #[ignore]
    fn real_server_token() {
        let token = std::env::var("MYTUNE_TEST_TOKEN").expect("MYTUNE_TEST_TOKEN");
        let device = std::env::var("MYTUNE_TEST_DEVICE").expect("MYTUNE_TEST_DEVICE");
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert!(verify(&token, &device, now).is_ok());
        assert!(verify(&token, "another", now).is_err());
    }

    #[test]
    fn refusal_reasons() {
        assert_eq!(refusal(Some("invalid")), Reject::Invalid);
        assert_eq!(refusal(Some("revoked")), Reject::Revoked);
        assert_eq!(refusal(Some("bound")), Reject::Bound);
        assert_eq!(refusal(Some("blocked")), Reject::Network, "unknown reason");
        assert_eq!(refusal(None), Reject::Network, "HTML block page or no body");
    }

    #[test]
    fn device_id_is_a_stable_hash() {
        let id = device_id().to_owned();
        assert_eq!(id.len(), 64);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(id, device_id());
        // the parts, read twice: the id must not hang on the cached value alone
        assert_eq!(board_serial(&smbios()), board_serial(&smbios()));
        assert_eq!(first_mac(), first_mac());
    }

    #[test]
    fn board_serial_from_raw_tables() {
        let mut raw = vec![0u8; 8];
        raw.extend([0, 5, 0, 0, 1]); // type 0, 5 bytes, one string
        raw.extend(b"BIOS\0\0");
        raw.extend([2, 8, 1, 0, 1, 2, 3, 4]); // type 2: serial is string 4
        raw.extend(b"Maker\0Board\0v1\0 SN123 \0\0");
        assert_eq!(board_serial(&raw), "SN123");
        assert_eq!(board_serial(&[]), "");
        assert_eq!(board_serial(&raw[..20]), "");
    }
}
