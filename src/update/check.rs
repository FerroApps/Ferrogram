//! The release check: one conditional GET of `releases/latest`, parsed into
//! a [`Release`], plus the small on-disk cache (ETag + last release) that
//! makes repeated checks cost a `304 Not Modified`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::http::{self, ApiReply, Endpoint};
use super::{CRATE, REPO, UpdateError, Version};

/// One downloadable file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A published (not draft, not pre-release) release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release page (release notes); always a github.com page of our repo.
    pub page: String,
    pub assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// The release page to open: GitHub's own link when it points into our
/// repository, otherwise the tag page built from scratch.
fn release_page(html_url: Option<&str>, tag: &str) -> String {
    let ours = format!("https://github.com/{REPO}/releases/");
    match html_url {
        Some(u) if u.starts_with(&ours) && !u.chars().any(|c| c.is_whitespace()) => u.to_owned(),
        _ if tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_+".contains(c)) =>
        {
            format!("{ours}tag/{tag}")
        }
        _ => super::releases_page(),
    }
}

/// Parse the `releases/latest` JSON. `Ok(None)` for drafts, pre-releases
/// and tags that are not a plain version.
pub fn parse_release(json: &[u8]) -> Result<Option<Release>, UpdateError> {
    let gh: GhRelease =
        serde_json::from_slice(json).map_err(|e| UpdateError::BadRelease(e.to_string()))?;
    if gh.draft || gh.prerelease {
        return Ok(None);
    }
    let Some(version) = Version::from_tag(&gh.tag_name) else {
        return Ok(None);
    };
    Ok(Some(Release {
        version,
        page: release_page(gh.html_url.as_deref(), &gh.tag_name),
        tag: gh.tag_name,
        assets: gh
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                size: a.size,
            })
            .collect(),
    }))
}

/// What this build can install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    LinuxX86_64,
    /// Ferrogram ships no build for this system.
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Self::LinuxX86_64
        } else {
            Self::Other
        }
    }
}

/// The tarball and its `.sha256` for `platform`, by the exact names the
/// release workflow publishes (`ferrogram-v<ver>-linux-x86_64.tar.gz`).
pub fn select_assets(release: &Release, platform: Platform) -> Option<(&Asset, &Asset)> {
    let name = match platform {
        Platform::LinuxX86_64 => format!("{CRATE}-v{}-linux-x86_64.tar.gz", release.version),
        Platform::Other => return None,
    };
    let sum = format!("{name}.sha256");
    let tarball = release.assets.iter().find(|a| a.name == name)?;
    let checksum = release.assets.iter().find(|a| a.name == sum)?;
    Some((tarball, checksum))
}

/// The last answer from GitHub, kept so the next check can send
/// `If-None-Match` and a 304 still knows the release.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckCache {
    pub etag: Option<String>,
    pub release: Option<Release>,
}

impl CheckCache {
    /// Missing or unreadable cache = empty (the next check is a full GET).
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .filter(|b| b.len() < 256 * 1024)
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Atomic write (temp + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, path)
    }
}

/// Ask GitHub for the latest release. Returns it (`None` when the latest
/// one is a draft/pre-release or unusable) and the cache to store.
pub fn check(
    endpoint: &Endpoint,
    cache: &CheckCache,
) -> Result<(Option<Release>, CheckCache), UpdateError> {
    // Only send the ETag when we still have the release it belongs to.
    let etag = cache.release.as_ref().and(cache.etag.as_deref());
    match http::get_release_json(endpoint, etag)? {
        ApiReply::NotModified => Ok((cache.release.clone(), cache.clone())),
        ApiReply::Fresh { body, etag } => {
            let release = parse_release(&body)?;
            let cache = CheckCache {
                etag: etag.filter(|_| release.is_some()),
                release: release.clone(),
            };
            Ok((release, cache))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(tag: &str, extra: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/FerroApps/Ferrogram/releases/tag/{tag}",
            "name":"Ferrogram {tag}","body":"notes",{extra}
            "assets":[
              {{"name":"ferrogram-v0.12.0-linux-x86_64.tar.gz","size":123,
                "browser_download_url":"https://github.com/FerroApps/Ferrogram/releases/download/v0.12.0/ferrogram-v0.12.0-linux-x86_64.tar.gz"}},
              {{"name":"ferrogram-v0.12.0-linux-x86_64.tar.gz.sha256","size":97,
                "browser_download_url":"https://github.com/FerroApps/Ferrogram/releases/download/v0.12.0/ferrogram-v0.12.0-linux-x86_64.tar.gz.sha256"}}
            ]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parses_a_published_release() {
        let r = parse_release(&json("v0.12.0", "")).unwrap().unwrap();
        assert_eq!(r.version, Version::new(0, 12, 0));
        assert_eq!(r.tag, "v0.12.0");
        assert_eq!(
            r.page,
            "https://github.com/FerroApps/Ferrogram/releases/tag/v0.12.0"
        );
        assert_eq!(r.assets.len(), 2);
    }

    #[test]
    fn drafts_prereleases_and_odd_tags_are_ignored() {
        assert_eq!(
            parse_release(&json("v0.12.0", r#""draft":true,"#)).unwrap(),
            None
        );
        assert_eq!(
            parse_release(&json("v0.12.0", r#""prerelease":true,"#)).unwrap(),
            None
        );
        assert_eq!(parse_release(&json("v0.12.0-rc.1", "")).unwrap(), None);
        assert_eq!(parse_release(&json("nightly", "")).unwrap(), None);
        assert!(parse_release(b"{not json").is_err());
        assert!(parse_release(b"{}").is_err());
    }

    #[test]
    fn foreign_release_pages_are_replaced() {
        assert_eq!(
            release_page(Some("https://evil.example/x"), "v1.0.0"),
            "https://github.com/FerroApps/Ferrogram/releases/tag/v1.0.0"
        );
        assert_eq!(
            release_page(None, "v1.0.0 ; rm"),
            "https://github.com/FerroApps/Ferrogram/releases/latest"
        );
    }

    #[test]
    fn selects_the_linux_tarball_and_checksum_by_exact_name() {
        let r = parse_release(&json("v0.12.0", "")).unwrap().unwrap();
        let (tar, sum) = select_assets(&r, Platform::LinuxX86_64).unwrap();
        assert_eq!(tar.name, "ferrogram-v0.12.0-linux-x86_64.tar.gz");
        assert_eq!(sum.name, "ferrogram-v0.12.0-linux-x86_64.tar.gz.sha256");
        assert_eq!(select_assets(&r, Platform::Other), None);

        // Assets of another version (a mislabelled release) do not match.
        let mut other = r.clone();
        other.version = Version::new(0, 13, 0);
        assert_eq!(select_assets(&other, Platform::LinuxX86_64), None);

        // A missing checksum means no self-update.
        let mut no_sum = r.clone();
        no_sum.assets.retain(|a| !a.name.ends_with(".sha256"));
        assert_eq!(select_assets(&no_sum, Platform::LinuxX86_64), None);
    }

    #[test]
    fn cache_round_trips_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/update-check.json");
        assert_eq!(CheckCache::load(&path), CheckCache::default());
        let cache = CheckCache {
            etag: Some("W/\"abc\"".into()),
            release: parse_release(&json("v0.12.0", "")).unwrap(),
        };
        cache.save(&path).unwrap();
        assert_eq!(CheckCache::load(&path), cache);
        std::fs::write(&path, b"garbage").unwrap();
        assert_eq!(CheckCache::load(&path), CheckCache::default());
    }
}
