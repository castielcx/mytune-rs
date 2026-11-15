//! SoundCloud through the API its own website uses (api-v2.soundcloud.com), not the official one: search,
//! the signed-in account's likes and playlists, and a playable stream address for a track.
//!
//! The website's `client_id` is read from its scripts and read again when SoundCloud refuses it (it changes
//! now and then). The account is the `oauth_token` cookie of a browser where the user is signed in, pasted
//! once and kept in the Windows credential store. Stream addresses are signed and expire, so they are asked
//! for right before playing. DRM tracks (encrypted HLS, SoundCloud Go+) are never played.

use serde_json::Value;
use std::sync::Mutex;
use std::time::Duration;

const API: &str = "https://api-v2.soundcloud.com";
const CRED: &str = "MyTune/SoundCloud";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub artist: String,
    /// Length in seconds.
    pub duration: f64,
    pub artwork: Option<String>,
    /// The transcoding address to resolve into a stream (`stream_url`); `None` when nothing playable.
    pub transcoding: Option<String>,
    pub auth: String,
    /// Only a 30 s preview is playable (Go+ track without a subscription).
    pub preview: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    pub id: u64,
    pub title: String,
    pub count: u64,
}

static CLIENT_ID: Mutex<Option<String>> = Mutex::new(None);

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(20)).user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)").build()
}

/// The website's client_id: the last script on soundcloud.com that names one.
fn fetch_client_id() -> Result<String, String> {
    let a = agent();
    let home = a.get("https://soundcloud.com/").call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
    let scripts: Vec<&str> = home.split("src=\"").skip(1).filter_map(|s| s.split('"').next()).filter(|s| s.starts_with("https://a-v2.sndcdn.com/assets/") && s.ends_with(".js")).collect();
    for src in scripts.iter().rev() {
        let Ok(js) = a.get(src).call().and_then(|r| Ok(r.into_string()?)) else { continue };
        if let Some(id) = find_client_id(&js) {
            return Ok(id);
        }
    }
    Err("SoundCloud's website did not give a client_id".into())
}

/// `client_id:"<32 letters or digits>"` in a script.
fn find_client_id(js: &str) -> Option<String> {
    js.match_indices("client_id:\"").find_map(|(i, m)| {
        let id: String = js[i + m.len()..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        (id.len() == 32 && js[i + m.len() + 32..].starts_with('"')).then_some(id)
    })
}

fn client_id(fresh: bool) -> Result<String, String> {
    let mut cached = CLIENT_ID.lock().unwrap_or_else(|p| p.into_inner());
    if fresh || cached.is_none() {
        *cached = Some(fetch_client_id()?);
    }
    Ok(cached.clone().unwrap_or_default())
}

/// GET a full address or an API path, with the client_id and, when given, the account token. A refused
/// client_id (401 without a token, 403) is read anew once.
fn get(path: &str, token: Option<&str>) -> Result<Value, String> {
    let url = if path.starts_with("http") { path.to_owned() } else { format!("{API}{path}") };
    for fresh in [false, true] {
        let sep = if url.contains('?') { '&' } else { '?' };
        let mut req = agent().get(&format!("{url}{sep}client_id={}", client_id(fresh)?));
        if let Some(t) = token {
            req = req.set("Authorization", &format!("OAuth {t}"));
        }
        match req.call() {
            Ok(r) => return serde_json::from_str(&r.into_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string()),
            Err(ureq::Error::Status(401, _)) if token.is_some() => return Err("SoundCloud did not accept the login token. Paste a new one.".into()),
            Err(ureq::Error::Status(401 | 403, _)) if !fresh => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("SoundCloud refused the request".into())
}

/// Every page of a list (`next_href`), at most `max` items.
fn pages(first: &str, token: Option<&str>, max: usize) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    let mut next = Some(first.to_owned());
    while let Some(url) = next.take() {
        let page = get(&url, token)?;
        out.extend(page["collection"].as_array().cloned().unwrap_or_default());
        if out.len() < max {
            next = page["next_href"].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        }
    }
    out.truncate(max);
    Ok(out)
}

/// Picks the stream to play: HLS AAC first (Windows plays it best), then HLS MP3; never encrypted ones.
fn parse_track(t: &Value) -> Option<Track> {
    let id = t["id"].as_u64()?;
    let title = t["title"].as_str()?.to_owned();
    let list = t["media"]["transcodings"].as_array().cloned().unwrap_or_default();
    let rank = |x: &Value| {
        let protocol = x["format"]["protocol"].as_str().unwrap_or("");
        let mime = x["format"]["mime_type"].as_str().unwrap_or("");
        match (protocol, mime) {
            ("hls", m) if m.starts_with("audio/mp4") => Some(0),
            ("hls", "audio/mpegurl") | ("hls", "audio/mpeg") => Some(1),
            ("progressive", _) => Some(2),
            _ => None, // encrypted-hls, ctr-encrypted-hls, ...: DRM
        }
    };
    let best = list.iter().filter_map(|x| Some((rank(x)?, x))).min_by_key(|(r, _)| *r).map(|(_, x)| x);
    let artist = t["publisher_metadata"]["artist"].as_str().filter(|s| !s.is_empty()).or(t["user"]["username"].as_str()).unwrap_or("").to_owned();
    let artwork = t["artwork_url"].as_str().or(t["user"]["avatar_url"].as_str()).map(|s| s.replace("-large.", "-t500x500."));
    Some(Track {
        id,
        title,
        artist,
        duration: t["duration"].as_f64().unwrap_or(0.0) / 1000.0,
        artwork,
        transcoding: best.and_then(|x| x["url"].as_str()).map(str::to_owned),
        auth: t["track_authorization"].as_str().unwrap_or("").to_owned(),
        preview: best.is_some_and(|x| x["snipped"].as_bool().unwrap_or(false)),
    })
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub fn search(query: &str, token: Option<&str>) -> Result<Vec<Track>, String> {
    let list = pages(&format!("/search/tracks?q={}&limit=50", enc(query)), token, 100)?;
    Ok(list.iter().filter_map(parse_track).collect())
}

fn me(token: &str) -> Result<u64, String> {
    get("/me", Some(token))?["id"].as_u64().ok_or_else(|| "SoundCloud did not say who is signed in".into())
}

/// The account's liked tracks, newest first.
pub fn likes(token: &str) -> Result<Vec<Track>, String> {
    let list = pages(&format!("/users/{}/track_likes?limit=200", me(token)?), Some(token), 2000)?;
    Ok(list.iter().filter_map(|x| parse_track(&x["track"])).collect())
}

/// The account's own and liked playlists and albums (not SoundCloud's generated ones).
pub fn playlists(token: &str) -> Result<Vec<Playlist>, String> {
    let list = pages("/me/library/all?limit=50", Some(token), 500)?;
    Ok(list
        .iter()
        .filter_map(|x| {
            let p = if x["playlist"].is_object() { &x["playlist"] } else { x };
            Some(Playlist { id: p["id"].as_u64()?, title: p["title"].as_str()?.to_owned(), count: p["track_count"].as_u64().unwrap_or(0) })
        })
        .collect())
}

/// A playlist's tracks in order. SoundCloud sends only the first few in full; the rest are fetched by id.
pub fn playlist_tracks(id: u64, token: Option<&str>) -> Result<Vec<Track>, String> {
    let p = get(&format!("/playlists/{id}"), token)?;
    let raw = p["tracks"].as_array().cloned().unwrap_or_default();
    let missing: Vec<u64> = raw.iter().filter(|t| t["title"].is_null()).filter_map(|t| t["id"].as_u64()).collect();
    let mut full: std::collections::HashMap<u64, Track> = raw.iter().filter_map(parse_track).map(|t| (t.id, t)).collect();
    for chunk in missing.chunks(50) {
        let ids: Vec<String> = chunk.iter().map(u64::to_string).collect();
        if let Value::Array(list) = get(&format!("/tracks?ids={}", ids.join("%2C")), token)? {
            full.extend(list.iter().filter_map(parse_track).map(|t| (t.id, t)));
        }
    }
    Ok(raw.iter().filter_map(|t| full.remove(&t["id"].as_u64()?)).collect())
}

/// The signed address of the track's audio (an HLS playlist or an MP3), valid for a short while.
pub fn stream_url(track: &Track, token: Option<&str>) -> Result<String, String> {
    let t = track.transcoding.as_deref().ok_or("This track cannot be played here (DRM or no stream)")?;
    let v = get(&format!("{t}?track_authorization={}", track.auth), token)?;
    v["url"].as_str().map(str::to_owned).ok_or_else(|| "SoundCloud gave no stream address".into())
}

/// Accepts the bare token or a pasted `oauth_token=...` cookie line.
pub fn clean_token(pasted: &str) -> String {
    let s = pasted.trim();
    let s = s.split(';').find_map(|p| p.trim().strip_prefix("oauth_token=")).unwrap_or(s);
    s.trim().trim_matches('"').to_owned()
}

pub fn load_token() -> Option<String> {
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let name: Vec<u16> = CRED.encode_utf16().chain([0]).collect();
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(windows::core::PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let c = &*cred;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let token = String::from_utf8(blob.to_vec()).ok();
        CredFree(cred as *const _);
        token.filter(|t| !t.is_empty())
    }
}

/// Keeps the token in the Windows credential store; an empty token removes it.
pub fn save_token(token: &str) -> Result<(), String> {
    use windows::Win32::Security::Credentials::{CredDeleteW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC};
    let mut name: Vec<u16> = CRED.encode_utf16().chain([0]).collect();
    unsafe {
        if token.is_empty() {
            let _ = CredDeleteW(windows::core::PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None);
            return Ok(());
        }
        let mut blob = token.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: windows::core::PWSTR(name.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        CredWriteW(&cred, 0).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(find_client_id(r#"x={client_id:"abcdefghijklmnopqrstuvwxyz012345",y:1}"#).as_deref(), Some("abcdefghijklmnopqrstuvwxyz012345"));
        assert_eq!(find_client_id(r#"client_id:"short""#), None);
        assert_eq!(clean_token(" oauth_token=2-123-abc; other=1 "), "2-123-abc");
        assert_eq!(clean_token("2-123-abc"), "2-123-abc");
        assert_eq!(enc("a b&c"), "a%20b%26c");
        let t: Value = serde_json::from_str(r#"{"id":5,"title":"T","duration":61000,"user":{"username":"u"},"artwork_url":"https://i1.sndcdn.com/a-large.jpg","track_authorization":"x",
            "media":{"transcodings":[{"url":"enc","format":{"protocol":"ctr-encrypted-hls","mime_type":"audio/mp4"},"snipped":false},
            {"url":"mp3","format":{"protocol":"hls","mime_type":"audio/mpegurl"},"snipped":false},
            {"url":"aac","format":{"protocol":"hls","mime_type":"audio/mp4; codecs=\"mp4a.40.2\""},"snipped":true}]}}"#).unwrap();
        let t = parse_track(&t).unwrap();
        assert_eq!((t.transcoding.as_deref(), t.preview, t.duration, t.artist.as_str()), (Some("aac"), true, 61.0, "u"));
        assert_eq!(t.artwork.as_deref(), Some("https://i1.sndcdn.com/a-t500x500.jpg"));
    }

    /// Asks SoundCloud for real: search, then a stream address for the first playable result.
    #[test]
    #[ignore]
    fn real_search() {
        let found = search("lofi", None).expect("search");
        println!("SC found={}", found.len());
        let t = found.iter().find(|t| t.transcoding.is_some()).expect("a playable track");
        let url = stream_url(t, None).expect("stream");
        println!("SC title={:?} stream={}", t.title, &url[..60.min(url.len())]);
        if let Some(token) = load_token() {
            println!("SC likes={} playlists={}", likes(&token).map(|l| l.len()).unwrap_or(0), playlists(&token).map(|l| l.len()).unwrap_or(0));
        }
    }
}
