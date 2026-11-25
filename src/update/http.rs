//! The updater's HTTP: `ureq` (blocking, rustls) with redirects followed by
//! hand so every hop is checked against an allow-list of hosts.
//!
//! Requests carry only `User-Agent: ferrogram/<version>`, `Accept` and, for
//! the release check, `If-None-Match`: no account data, no telemetry, no
//! machine id. URLs are logged without their query string (GitHub's asset
//! redirects carry signed tokens there).

use std::io::Read;
use std::time::Duration;

use super::{CRATE, MAX_DOWNLOAD, UpdateError};

/// Hosts the release API may live on (and redirect to).
pub const API_HOSTS: &[&str] = &["api.github.com"];
/// Hosts release downloads may come from (GitHub redirects asset links to
/// its storage hosts).
pub const DOWNLOAD_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_REDIRECTS: usize = 6;
const API_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A stalled download (no byte for this long) fails.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Whole download, however slow the line.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Where the release check goes. `FERRO_RELEASE_API` (the same variable the
/// `install.sh` tests use) replaces the whole "latest release" URL; with it,
/// plain `http://` to the loopback interface is accepted so tests can run a
/// local server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub latest_url: String,
    pub overridden: bool,
}

impl Endpoint {
    pub fn github() -> Self {
        Self {
            latest_url: format!(
                "https://api.github.com/repos/{}/releases/latest",
                super::REPO
            ),
            overridden: false,
        }
    }

    pub fn from_env() -> Self {
        Self::from_override(std::env::var("FERRO_RELEASE_API").ok())
    }

    pub fn from_override(value: Option<String>) -> Self {
        match value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) {
            Some(url) => Self {
                latest_url: url,
                overridden: true,
            },
            None => Self::github(),
        }
    }

    pub fn api_policy(&self) -> Policy {
        Policy {
            hosts: API_HOSTS,
            any_https: self.overridden,
            loopback: self.overridden,
        }
    }

    pub fn download_policy(&self) -> Policy {
        Policy {
            hosts: DOWNLOAD_HOSTS,
            any_https: false,
            loopback: self.overridden,
        }
    }
}

/// Which URLs a request (and each redirect hop) may go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub hosts: &'static [&'static str],
    /// Any HTTPS host (an explicit `FERRO_RELEASE_API` endpoint).
    pub any_https: bool,
    /// `http://` or `https://` to 127.0.0.1 / ::1 / localhost (tests only).
    pub loopback: bool,
}

/// Scheme and host of `url`; `None` when it does not parse or carries
/// credentials (`user@host`).
fn split_url(url: &str) -> Option<(String, String)> {
    let uri: ureq::http::Uri = url.parse().ok()?;
    if uri.authority()?.as_str().contains('@') {
        return None;
    }
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let host = uri.host()?.to_ascii_lowercase();
    Some((scheme, host))
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

impl Policy {
    pub fn allows(&self, url: &str) -> bool {
        let Some((scheme, host)) = split_url(url) else {
            return false;
        };
        if self.loopback && is_loopback(&host) && (scheme == "http" || scheme == "https") {
            return true;
        }
        scheme == "https" && (self.any_https || self.hosts.contains(&host.as_str()))
    }
}

/// The URL without query string or fragment, for logs.
pub fn loggable(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// Resolve a `Location` header against the URL that sent it (absolute URLs
/// and absolute paths only).
pub fn resolve_location(base: &str, location: &str) -> Option<String> {
    let location = location.trim();
    if location.contains("://") {
        return Some(location.to_owned());
    }
    if location.starts_with('/') && !location.starts_with("//") {
        let uri: ureq::http::Uri = base.parse().ok()?;
        return Some(format!(
            "{}://{}{location}",
            uri.scheme_str()?,
            uri.authority()?.as_str()
        ));
    }
    None
}

fn map_err(e: ureq::Error) -> UpdateError {
    // Never format the ureq error itself: some variants carry the URL.
    let what = match e {
        ureq::Error::Timeout(_) => "timed out".to_owned(),
        ureq::Error::HostNotFound => "host not found".to_owned(),
        ureq::Error::ConnectionFailed => "connection failed".to_owned(),
        ureq::Error::Io(e) => format!("{}", e.kind()),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "TLS error".to_owned(),
        ureq::Error::BodyExceedsLimit(_) => return UpdateError::TooLarge,
        ureq::Error::StatusCode(code) => return UpdateError::Http(code),
        _ => "request failed".to_owned(),
    };
    UpdateError::Network(what)
}

fn agent(global: Duration, read: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(global))
        .timeout_per_read(read)
        .user_agent(format!("{CRATE}/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

pub type Response = ureq::http::Response<ureq::Body>;

/// GET `url`, following up to six redirects, each checked against `policy`.
fn get(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(&str, &str)],
    policy: Policy,
) -> Result<Response, UpdateError> {
    let mut current = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        if !policy.allows(&current) {
            tracing::warn!(url = loggable(&current), "update: URL not allowed");
            return Err(UpdateError::BlockedUrl);
        }
        let mut request = agent.get(&current);
        for (k, v) in headers {
            request = request.header(*k, *v);
        }
        let response = request.call().map_err(map_err)?;
        let status = response.status();
        if status.is_redirection() && status.as_u16() != 304 {
            let next = response
                .headers()
                .get("location")
                .and_then(|l| l.to_str().ok())
                .and_then(|l| resolve_location(&current, l))
                .ok_or(UpdateError::BadRelease(
                    "redirect without a location".into(),
                ))?;
            tracing::debug!(
                from = loggable(&current),
                to = loggable(&next),
                "update: redirect"
            );
            current = next;
            continue;
        }
        return Ok(response);
    }
    Err(UpdateError::Network("too many redirects".into()))
}

/// The release API's answer to a conditional GET.
#[derive(Debug)]
pub enum ApiReply {
    /// 200: the body and the new ETag.
    Fresh { body: Vec<u8>, etag: Option<String> },
    /// 304: what we have is still current.
    NotModified,
}

/// GET the "latest release" JSON (1 MB cap). 403/429 map to
/// [`UpdateError::RateLimited`].
pub fn get_release_json(endpoint: &Endpoint, etag: Option<&str>) -> Result<ApiReply, UpdateError> {
    let agent = agent(API_TIMEOUT, None);
    let mut headers = vec![
        ("Accept", "application/vnd.github+json"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ];
    if let Some(tag) = etag {
        headers.push(("If-None-Match", tag));
    }
    let response = get(
        &agent,
        &endpoint.latest_url,
        &headers,
        endpoint.api_policy(),
    )?;
    match response.status().as_u16() {
        200 => {
            let etag = response
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let body = response
                .into_body()
                .with_config()
                .limit(1024 * 1024)
                .read_to_vec()
                .map_err(map_err)?;
            Ok(ApiReply::Fresh { body, etag })
        }
        304 => Ok(ApiReply::NotModified),
        403 | 429 => Err(UpdateError::RateLimited),
        code => Err(UpdateError::Http(code)),
    }
}

/// Download a small file (a checksum) completely, at most `limit` bytes.
pub fn get_small(url: &str, policy: Policy, limit: u64) -> Result<Vec<u8>, UpdateError> {
    let agent = agent(API_TIMEOUT, None);
    let response = get(
        &agent,
        url,
        &[("Accept", "application/octet-stream")],
        policy,
    )?;
    match response.status().as_u16() {
        200 => response
            .into_body()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(map_err),
        code => Err(UpdateError::Http(code)),
    }
}

/// Start a large download: the body reader and the announced length.
pub fn get_stream(
    url: &str,
    policy: Policy,
) -> Result<(Box<dyn Read + Send>, Option<u64>), UpdateError> {
    let agent = agent(DOWNLOAD_TIMEOUT, Some(READ_TIMEOUT));
    let response = get(
        &agent,
        url,
        &[("Accept", "application/octet-stream")],
        policy,
    )?;
    match response.status().as_u16() {
        200 => {
            let len = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            if len.is_some_and(|l| l > MAX_DOWNLOAD) {
                return Err(UpdateError::TooLarge);
            }
            Ok((Box::new(response.into_body().into_reader()), len))
        }
        code => Err(UpdateError::Http(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_policy_allows_only_https_github_hosts() {
        let api = Endpoint::github().api_policy();
        assert!(api.allows("https://api.github.com/repos/FerroApps/Ferrogram/releases/latest"));
        assert!(!api.allows("http://api.github.com/repos/x"));
        assert!(!api.allows("https://github.com/x"));
        assert!(!api.allows("http://127.0.0.1:8080/latest"));

        let dl = Endpoint::github().download_policy();
        for ok in [
            "https://github.com/FerroApps/Ferrogram/releases/download/v1.0.0/a.tar.gz",
            "https://objects.githubusercontent.com/x?token=1",
            "https://release-assets.githubusercontent.com/x?sig=2",
            "https://GitHub.com/x",
        ] {
            assert!(dl.allows(ok), "{ok}");
        }
        for bad in [
            "http://github.com/x",
            "https://github.com.evil.example/x",
            "https://evil.example/github.com",
            "https://user@github.com/x",
            "ftp://github.com/x",
            "file:///etc/passwd",
            "https://127.0.0.1/x",
            "not a url",
        ] {
            assert!(!dl.allows(bad), "{bad}");
        }
    }

    #[test]
    fn override_allows_loopback_for_tests() {
        let e = Endpoint::from_override(Some("http://127.0.0.1:9/latest".into()));
        assert!(e.overridden);
        assert!(e.api_policy().allows("http://127.0.0.1:9/latest"));
        assert!(e.download_policy().allows("http://localhost:9/a.tar.gz"));
        assert!(!e.download_policy().allows("http://example.com/a.tar.gz"));
        assert!(!e.download_policy().allows("https://example.com/a.tar.gz"));
        assert_eq!(
            Endpoint::from_override(Some("  ".into())),
            Endpoint::github()
        );
        assert_eq!(Endpoint::from_override(None), Endpoint::github());
    }

    #[test]
    fn logs_never_carry_query_tokens() {
        assert_eq!(
            loggable("https://objects.githubusercontent.com/a/b?X-Amz-Signature=secret#f"),
            "https://objects.githubusercontent.com/a/b"
        );
        assert_eq!(loggable("https://github.com/a"), "https://github.com/a");
    }

    #[test]
    fn locations_resolve() {
        assert_eq!(
            resolve_location(
                "https://github.com/a/b",
                "https://objects.githubusercontent.com/x?y"
            )
            .as_deref(),
            Some("https://objects.githubusercontent.com/x?y")
        );
        assert_eq!(
            resolve_location("https://github.com:443/a/b", "/c/d").as_deref(),
            Some("https://github.com:443/c/d")
        );
        assert_eq!(
            resolve_location("https://github.com/a", "//evil.example/x"),
            None
        );
        assert_eq!(resolve_location("https://github.com/a", "relative"), None);
    }
}
