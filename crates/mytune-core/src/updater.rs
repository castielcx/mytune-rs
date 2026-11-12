//! Settings > Updates, "Check now" (C# `Updater.CheckAsync`): reads `version.json` from the release feed and
//! says whether a newer MyTune exists. Nothing about the user is sent; it is a plain download.
//!
//! NOT ported: download, SHA-256 check and the silent install (the feed's setup installs the C# app, which
//! would replace this one), the update banner, the 6-hourly background check and release notes.

use serde::Deserialize;
use std::time::Duration;

/// Version of the MyTune being ported (C# `MyTune.csproj`). The feed is compared against this.
pub const APP_VERSION: &str = "2.4.3";
pub const FEED_URL: &str = "https://github.com/castielcx/MyTune/releases/latest/download/version.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub notes: String,
}

#[derive(Deserialize)]
struct Feed {
    version: Option<String>,
    url: Option<String>,
    sha256: Option<String>,
    notes: Option<String>,
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

/// Validates the feed text. `Ok(Some)` = newer version, `Ok(None)` = up to date.
pub fn evaluate(feed_json: &str, current: &str) -> Result<Option<UpdateInfo>, String> {
    let feed: Feed = serde_json::from_str(feed_json).map_err(|_| "Empty update feed.".to_string())?;
    let latest = feed.version.as_deref().and_then(parse_version).ok_or("Update feed has no valid version.")?;
    let url = feed.url.unwrap_or_default();
    if !url.to_lowercase().starts_with("https://") {
        return Err("Update download must use https.".into());
    }
    let sha = feed.sha256.unwrap_or_default();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("Update feed has no valid SHA-256.".into());
    }
    let cur = parse_version(current).ok_or("Bad current version.")?;
    Ok(newer(&latest, &cur).then(|| UpdateInfo { version: feed.version.unwrap_or_default(), url, sha256: sha.to_lowercase(), notes: feed.notes.unwrap_or_default() }))
}

/// Downloads the feed (15 s) and compares it with this version.
pub fn check() -> Result<Option<UpdateInfo>, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let text = agent.get(FEED_URL).call().map_err(|e| e.to_string())?.into_string().map_err(|e| e.to_string())?;
    evaluate(&text, APP_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn feed(v: &str, url: &str, sha: &str) -> String {
        format!(r#"{{"version":"{v}","url":"{url}","sha256":"{sha}","notes":"n"}}"#)
    }

    #[test]
    fn newer_older_and_equal() {
        assert!(evaluate(&feed("2.4.4", "https://x/s.exe", SHA), "2.4.3").unwrap().is_some());
        assert!(evaluate(&feed("2.5.0", "https://x/s.exe", SHA), "2.4.3").unwrap().is_some());
        assert!(evaluate(&feed("2.4.3", "https://x/s.exe", SHA), "2.4.3").unwrap().is_none());
        assert!(evaluate(&feed("2.4.2", "https://x/s.exe", SHA), "2.4.3").unwrap().is_none());
        assert!(evaluate(&feed("2.4", "https://x/s.exe", SHA), "2.4.3").unwrap().is_none(), "2.4 == 2.4.0 < 2.4.3");
        assert!(evaluate(&feed("v10.0.0", "https://x/s.exe", SHA), "2.4.3").unwrap().is_some());
    }

    #[test]
    fn bad_feeds_are_refused_with_the_csharp_messages() {
        assert_eq!(evaluate("{}", "2.4.3").unwrap_err(), "Update feed has no valid version.");
        assert_eq!(evaluate(&feed("2.5.0", "http://x/s.exe", SHA), "2.4.3").unwrap_err(), "Update download must use https.");
        assert_eq!(evaluate(&feed("2.5.0", "https://x/s.exe", "abc"), "2.4.3").unwrap_err(), "Update feed has no valid SHA-256.");
        assert_eq!(evaluate("not json", "2.4.3").unwrap_err(), "Empty update feed.");
        let up = evaluate(&feed("2.5.0", "https://x/s.exe", &SHA.to_uppercase()), "2.4.3").unwrap().unwrap();
        assert_eq!(up.sha256, SHA, "hash is stored lower case");
    }
}
