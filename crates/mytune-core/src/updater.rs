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
    use std::time::Duration;

    pub const FEED_URL: &str = "https://github.com/castielcx/mytune-rs/releases/latest/download/version.json";
    pub const URL_PREFIX: &str = "https://github.com/castielcx/mytune-rs/releases/download/";

    /// PLACEHOLDER: the owner's key step replaces the right-hand side with the base64 public key.
    const PLACEHOLDER_NO_UPDATE_KEY: &str = "";
    const UPDATE_PUBLIC_KEY: &str = PLACEHOLDER_NO_UPDATE_KEY;
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
        if !url.starts_with(URL_PREFIX) || ["..", "?", "#", "\\", "@"].iter().any(|bad| url.contains(bad)) {
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
    pub fn check() -> Result<Option<UpdateInfo>, String> {
        let key = parse_key(UPDATE_PUBLIC_KEY)?;
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
        let text = agent.get(&feed_url()).call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
        evaluate(&text, &current_version(), &key)
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
    }
}
