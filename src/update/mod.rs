//! In-app updater (docs/architecture.md §11.8 "Updates").
//!
//! The app asks GitHub Releases of its own repository for the latest
//! release (one small, anonymous request with an ETag), shows a banner when
//! a newer version exists and, for an `install.sh` install, downloads,
//! verifies and installs it itself. Other install methods are told how to
//! update (cargo) or pointed at the release page.
//!
//! Everything here is plain Rust with no iced types: the app drives it
//! through [`state::Updater`] (pure) and runs the blocking parts
//! ([`check::check`], [`install::install`]) on their own threads.

pub mod check;
pub mod detect;
pub mod extract;
pub mod http;
pub mod install;
pub mod restart;
pub mod schedule;
pub mod state;
pub mod version;

pub use detect::InstallMethod;
pub use version::Version;

/// The GitHub repository releases come from.
pub const REPO: &str = "FerroApps/Ferrogram";
/// Crate / binary name (asset names, install paths).
pub const CRATE: &str = "ferrogram";
/// Hard cap for a downloaded release tarball.
pub const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;

/// The command a `cargo install` user runs to update.
pub fn cargo_install_command() -> String {
    format!("cargo install --git https://github.com/{REPO} --locked --force")
}

/// The releases page (fallback when a release has no usable page link).
pub fn releases_page() -> String {
    format!("https://github.com/{REPO}/releases/latest")
}

/// Why a check or an update failed. The text is shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Connection, TLS or timeout problem (details already stripped of URLs).
    Network(String),
    /// An unexpected HTTP status.
    Http(u16),
    /// GitHub's anonymous rate limit (403/429).
    RateLimited,
    /// The release metadata or checksum file could not be understood.
    BadRelease(String),
    /// The release has no build for this platform.
    NoAsset,
    /// A URL outside the allowed hosts (or not HTTPS).
    BlockedUrl,
    TooLarge,
    /// The downloaded file does not match the published SHA-256.
    Checksum,
    /// The archive contains something we refuse to extract.
    UnsafeArchive(String),
    /// A file system step failed; nothing was changed.
    Io(String),
    Cancelled,
    /// Self-update is not possible for this install method.
    NotSupported,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "network error ({e})"),
            Self::Http(code) => write!(f, "GitHub answered HTTP {code}"),
            Self::RateLimited => f.write_str("GitHub's rate limit was reached; try again later"),
            Self::BadRelease(e) => write!(f, "unexpected release data ({e})"),
            Self::NoAsset => f.write_str("the release has no build for this system"),
            Self::BlockedUrl => f.write_str("the download link points outside GitHub"),
            Self::TooLarge => f.write_str("the download is larger than expected"),
            Self::Checksum => {
                f.write_str("the download is corrupt (checksum mismatch); nothing was changed")
            }
            Self::UnsafeArchive(e) => write!(f, "the archive was rejected ({e})"),
            Self::Io(e) => write!(f, "could not install ({e}); nothing was changed"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::NotSupported => f.write_str("this installation cannot update itself"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl UpdateError {
    pub(crate) fn io(what: &str, e: &std::io::Error) -> Self {
        Self::Io(format!("{what}: {}", e.kind()))
    }
}
