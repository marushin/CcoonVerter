//! GitHub release check (port of `update_checker.py`) with a 24 h cache.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::config;

/// Information about a newer release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    /// Normalised version (e.g. `0.9.1`).
    pub version: String,
    /// Raw tag as published (e.g. `v0.9.1`).
    pub tag: String,
    pub release_notes: String,
    pub html_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("network error: {0}")]
    Http(String),
    #[error("unparsable version '{0}'")]
    Version(String),
}

/// Parse tags like `0.8.7`, `v0.9.0`, `v.0.8.5`, `V1.2`, `release-1.0.0-beta.1`.
/// Leading non-digits are dropped; if strict semver parsing fails, the
/// numeric dot-separated components are taken and padded to three.
pub fn parse_version(tag: &str) -> Option<Version> {
    let s = tag.trim();
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let s = &s[start..];
    if let Ok(v) = Version::parse(s) {
        return Some(v);
    }
    let mut nums = Vec::new();
    for part in s.split('.') {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        nums.push(digits.parse::<u64>().ok()?);
        if digits.len() != part.len() {
            break; // e.g. "0-beta": stop after the numeric prefix
        }
    }
    if nums.is_empty() {
        return None;
    }
    nums.resize(3, 0);
    Some(Version::new(nums[0], nums[1], nums[2]))
}

/// True if `latest` is strictly newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

/// Query the GitHub API for the latest release (no cache).
/// Returns `Ok(None)` when the current version is up to date.
pub fn fetch_latest(current: &str) -> Result<Option<ReleaseInfo>, UpdateError> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(config::UPDATE_TIMEOUT_SECS)))
        .user_agent(config::user_agent())
        .build();
    let agent = ureq::Agent::new_with_config(config);
    log::info!("Checking for updates at {}", config::GITHUB_API_LATEST_URL);
    let mut resp = agent
        .get(config::GITHUB_API_LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    let rel: GhRelease = resp
        .body_mut()
        .read_json()
        .map_err(|e| UpdateError::Http(e.to_string()))?;
    release_if_newer(rel, current)
}

fn release_if_newer(rel: GhRelease, current: &str) -> Result<Option<ReleaseInfo>, UpdateError> {
    let latest =
        parse_version(&rel.tag_name).ok_or_else(|| UpdateError::Version(rel.tag_name.clone()))?;
    let cur = parse_version(current).ok_or_else(|| UpdateError::Version(current.to_string()))?;
    log::info!("Current version: {cur}, latest version: {latest}");
    if latest > cur {
        Ok(Some(ReleaseInfo {
            version: latest.to_string(),
            tag: rel.tag_name,
            release_notes: rel
                .body
                .filter(|b| !b.trim().is_empty())
                .unwrap_or_else(|| "No release notes provided.".into()),
            html_url: rel
                .html_url
                .unwrap_or_else(|| config::GITHUB_RELEASES_URL.to_string()),
        }))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    /// Version that performed the check (a cache from an older build is ignored).
    checked_by: String,
    latest: Option<ReleaseInfo>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Cached check: if `cache_file` was written less than `max_age` ago by this
/// version, its answer is reused; otherwise GitHub is queried and the cache
/// rewritten. Network errors are returned (and not cached).
pub fn check_for_update_cached(
    current: &str,
    cache_file: &Path,
    max_age: Duration,
) -> Result<Option<ReleaseInfo>, UpdateError> {
    check_with(current, cache_file, max_age, now_secs(), fetch_latest)
}

/// [`check_for_update_cached`] with the defaults (24 h).
pub fn check_for_update(
    current: &str,
    cache_file: &Path,
) -> Result<Option<ReleaseInfo>, UpdateError> {
    check_for_update_cached(
        current,
        cache_file,
        Duration::from_secs(config::UPDATE_CACHE_SECS),
    )
}

fn check_with(
    current: &str,
    cache_file: &Path,
    max_age: Duration,
    now: u64,
    fetch: impl FnOnce(&str) -> Result<Option<ReleaseInfo>, UpdateError>,
) -> Result<Option<ReleaseInfo>, UpdateError> {
    if let Ok(text) = std::fs::read_to_string(cache_file) {
        if let Ok(c) = serde_json::from_str::<Cache>(&text) {
            let fresh = now.saturating_sub(c.checked_at) < max_age.as_secs() && c.checked_at <= now;
            if fresh && c.checked_by == current {
                log::debug!("Using cached update check");
                // Re-evaluate in case the cached release is not newer for us.
                return Ok(c.latest.filter(|r| is_newer(&r.version, current)));
            }
        }
    }
    let latest = fetch(current)?;
    let cache = Cache {
        checked_at: now,
        checked_by: current.to_string(),
        latest: latest.clone(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&cache) {
        if let Err(e) = crate::paths::write_atomic(cache_file, text.as_bytes()) {
            log::debug!("Could not write update cache: {e}");
        }
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn tolerant_version_parsing() {
        assert_eq!(parse_version("0.8.7"), Some(Version::new(0, 8, 7)));
        assert_eq!(parse_version("v0.9.0"), Some(Version::new(0, 9, 0)));
        assert_eq!(parse_version("v.0.8.5"), Some(Version::new(0, 8, 5)));
        assert_eq!(parse_version("V1.2"), Some(Version::new(1, 2, 0)));
        assert_eq!(parse_version(" 2 "), Some(Version::new(2, 0, 0)));
        assert_eq!(
            parse_version("v1.0.0-beta.1"),
            Some(Version::parse("1.0.0-beta.1").unwrap())
        );
        assert_eq!(parse_version("0.9-rc"), Some(Version::new(0, 9, 0)));
        assert_eq!(parse_version("latest"), None);
    }

    #[test]
    fn version_comparison_like_python() {
        assert!(is_newer("0.8.7", "v.0.8.5"));
        assert!(!is_newer("v.0.8.5", "0.8.7"));
        assert!(is_newer("v0.9.0", "0.8.7"));
        assert!(!is_newer("0.9.0", "0.9.0"));
        assert!(is_newer("0.10.0", "0.9.0")); // numeric, not lexicographic
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
        assert!(!is_newer("garbage", "0.9.0"));
    }

    fn rel(tag: &str) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            body: None,
            html_url: None,
        }
    }

    #[test]
    fn release_mapping() {
        let r = release_if_newer(rel("v.0.9.1"), "0.9.0").unwrap().unwrap();
        assert_eq!(r.version, "0.9.1");
        assert_eq!(r.tag, "v.0.9.1");
        assert_eq!(r.release_notes, "No release notes provided.");
        assert_eq!(r.html_url, config::GITHUB_RELEASES_URL);
        assert!(release_if_newer(rel("v0.8.7"), "0.9.0").unwrap().is_none());
        assert!(release_if_newer(rel("nope"), "0.9.0").is_err());
    }

    /// Everyone on an older release must be offered 1.0.0, whatever form the
    /// tag or their own version takes.
    #[test]
    fn one_point_zero_is_newer_than_every_earlier_release() {
        for old in [
            "0.13.0", "0.12.0", "0.11.0", "0.10.0", "0.9.0", "v0.9.0", "v.0.8.5", "0.8.7",
        ] {
            assert!(is_newer("v1.0.0", old), "{old}");
            assert!(!is_newer(old, "1.0.0"), "{old}");
        }
        assert!(!is_newer("v1.0.0", "1.0.0"));
        let r = release_if_newer(rel("v1.0.0"), "0.9.0").unwrap().unwrap();
        assert_eq!(r.version, "1.0.0");
        assert!(release_if_newer(rel("v1.0.0"), "1.0.0").unwrap().is_none());
    }

    #[test]
    fn cache_is_used_within_max_age() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("c").join("update.json");
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(Some(ReleaseInfo {
                version: "0.9.5".into(),
                tag: "v0.9.5".into(),
                release_notes: String::new(),
                html_url: String::new(),
            }))
        };
        let day = Duration::from_secs(86_400);
        let r1 = check_with("0.9.0", &cache, day, 1_000_000, fetch).unwrap();
        assert!(r1.is_some());
        assert_eq!(calls.get(), 1);
        // Within 24 h: cached, no fetch.
        let r2 = check_with("0.9.0", &cache, day, 1_000_000 + 3600, |_| {
            panic!("should not fetch")
        })
        .unwrap();
        assert_eq!(r2, r1);
        // After 24 h: fetch again.
        let r3 = check_with("0.9.0", &cache, day, 1_000_000 + 90_000, |_| Ok(None)).unwrap();
        assert!(r3.is_none());
        // A different (newer) app version ignores the cache.
        let r4 = check_with("0.9.5", &cache, day, 1_000_000 + 90_001, |_| Ok(None)).unwrap();
        assert!(r4.is_none());
    }

    /// Live network test; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_github_query() {
        let r = fetch_latest("0.0.1");
        assert!(r.is_ok(), "{r:?}");
    }
}
