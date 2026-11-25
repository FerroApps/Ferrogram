//! The updater as the UI sees it (pure, no I/O): what the banner and the
//! Settings section show, and which work to start.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::check::Release;
use super::install::Progress;
use super::{InstallMethod, UpdateError, Version};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking {
        manual: bool,
    },
    /// A check found nothing newer.
    UpToDate,
    /// A manual check failed (Settings shows it; timed checks fail silently).
    CheckFailed(String),
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Verifying,
    Installing,
    /// Installed; the new version runs after a restart.
    Installed(Version),
    InstallFailed(String),
}

/// What the banner shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Banner {
    /// A newer version; what [Update] can do depends on the install method.
    Available {
        version: Version,
        action: Action,
    },
    Downloading {
        version: Version,
        percent: Option<u8>,
    },
    Verifying(Version),
    Installing(Version),
    Installed(Version),
    Failed(String),
}

/// How this install gets the new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Download and install in the app.
    SelfUpdate,
    /// Show the `cargo install` command.
    Cargo(String),
    /// Only open the release page.
    OpenPage,
}

#[derive(Debug, Clone)]
pub struct Updater {
    pub current: Version,
    /// `None` until detected (off the UI thread, at startup).
    pub method: Option<InstallMethod>,
    /// "Check for updates automatically" (Settings).
    pub auto: bool,
    /// Timed checks may run at all (not mock/screenshot/debug).
    pub auto_allowed: bool,
    /// The network may be used (false in mock mode).
    pub network: bool,
    /// The newest release when it is newer than `current`.
    pub latest: Option<Release>,
    /// The version whose banner the user closed (persisted).
    pub dismissed: Option<Version>,
    pub phase: Phase,
    /// "Later" on "Restart to finish": hidden for this session.
    pub restart_later: bool,
    /// Bumped to invalidate pending timed checks.
    pub schedule_gen: u64,
    cancel: Option<Arc<AtomicBool>>,
}

/// What [`Updater::begin_check`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStart {
    Start,
    /// Busy, or the network is off (mock): nothing to do.
    Skip,
}

impl Updater {
    pub fn new(auto: bool, dismissed: Option<Version>, auto_allowed: bool, network: bool) -> Self {
        Self {
            current: Version::current(),
            method: None,
            auto,
            auto_allowed,
            network,
            latest: None,
            dismissed,
            phase: Phase::Idle,
            restart_later: false,
            schedule_gen: 0,
            cancel: None,
        }
    }

    /// Timed checks should be scheduled.
    pub fn auto_enabled(&self) -> bool {
        self.auto && self.auto_allowed && self.network
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking { .. }
                | Phase::Downloading { .. }
                | Phase::Verifying
                | Phase::Installing
        )
    }

    pub fn installing(&self) -> bool {
        matches!(
            self.phase,
            Phase::Downloading { .. } | Phase::Verifying | Phase::Installing
        )
    }

    pub fn action(&self) -> Action {
        match &self.method {
            Some(InstallMethod::InstallSh { .. }) => Action::SelfUpdate,
            Some(InstallMethod::Cargo) => Action::Cargo(super::cargo_install_command()),
            _ => Action::OpenPage,
        }
    }

    pub fn begin_check(&mut self, manual: bool) -> CheckStart {
        if !self.network || self.busy() {
            return CheckStart::Skip;
        }
        // Once installed, a check would only say what we already know.
        if matches!(self.phase, Phase::Installed(_)) {
            return CheckStart::Skip;
        }
        self.phase = Phase::Checking { manual };
        CheckStart::Start
    }

    pub fn on_checked(&mut self, manual: bool, result: Result<Option<Release>, UpdateError>) {
        if !matches!(self.phase, Phase::Checking { .. }) {
            return;
        }
        match result {
            Ok(release) => {
                self.latest = release.filter(|r| r.version > self.current);
                self.phase = match &self.latest {
                    Some(_) => Phase::Idle,
                    None => Phase::UpToDate,
                };
            }
            Err(e) => {
                tracing::info!(error = %e, manual, "update check failed");
                self.phase = if manual {
                    Phase::CheckFailed(e.to_string())
                } else {
                    Phase::Idle
                };
            }
        }
    }

    /// The banner, if one should be shown.
    pub fn banner(&self) -> Option<Banner> {
        let version = self.latest.as_ref().map(|r| r.version);
        match (&self.phase, version) {
            (Phase::Downloading { done, total }, Some(version)) => Some(Banner::Downloading {
                version,
                percent: total
                    .filter(|t| *t > 0)
                    .map(|t| u8::try_from((done.saturating_mul(100) / t).min(100)).unwrap_or(100)),
            }),
            (Phase::Verifying, Some(v)) => Some(Banner::Verifying(v)),
            (Phase::Installing, Some(v)) => Some(Banner::Installing(v)),
            (Phase::Installed(v), _) => (!self.restart_later).then_some(Banner::Installed(*v)),
            (_, Some(v)) if self.dismissed.is_some_and(|d| d >= v) => None,
            (Phase::InstallFailed(e), Some(_)) => Some(Banner::Failed(e.clone())),
            (_, Some(version)) => Some(Banner::Available {
                version,
                action: self.action(),
            }),
            _ => None,
        }
    }

    /// [x] on the banner (or "Later"). Returns the version to persist as
    /// dismissed, if any.
    pub fn dismiss(&mut self) -> Option<Version> {
        if self.installing() {
            return None;
        }
        if matches!(self.phase, Phase::Installed(_)) {
            self.restart_later = true;
            return None;
        }
        if matches!(self.phase, Phase::InstallFailed(_)) {
            self.phase = Phase::Idle;
        }
        let v = self.latest.as_ref()?.version;
        self.dismissed = Some(v);
        Some(v)
    }

    /// [Update]: the release to install and its cancel flag, when allowed.
    pub fn start_install(&mut self) -> Option<(Release, Arc<AtomicBool>)> {
        if !self.network || self.busy() || self.action() != Action::SelfUpdate {
            return None;
        }
        if matches!(self.phase, Phase::Installed(_)) {
            return None;
        }
        let release = self.latest.clone()?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.phase = Phase::Downloading {
            done: 0,
            total: None,
        };
        Some((release, cancel))
    }

    pub fn on_progress(&mut self, p: Progress) {
        if !self.installing() {
            return;
        }
        self.phase = match p {
            Progress::Downloading { done, total } => Phase::Downloading { done, total },
            Progress::Verifying => Phase::Verifying,
            Progress::Installing => Phase::Installing,
        };
    }

    /// Cancel a download (also called on quit). Only downloads stop: the
    /// final renames are quick and atomic.
    pub fn cancel(&mut self) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
        }
    }

    pub fn can_cancel(&self) -> bool {
        matches!(self.phase, Phase::Downloading { .. })
    }

    pub fn on_installed(&mut self, result: Result<Version, UpdateError>) {
        self.cancel = None;
        self.phase = match result {
            Ok(v) => {
                self.restart_later = false;
                Phase::Installed(v)
            }
            Err(UpdateError::Cancelled) => Phase::Idle,
            Err(e) => {
                tracing::warn!(error = %e, "update failed");
                Phase::InstallFailed(e.to_string())
            }
        };
    }

    /// The binary to start for "Restart now".
    pub fn restart_binary(&self) -> Option<std::path::PathBuf> {
        match (&self.phase, &self.method) {
            (Phase::Installed(_), Some(InstallMethod::InstallSh { bin_dir, .. })) => {
                Some(bin_dir.join(super::CRATE))
            }
            _ => None,
        }
    }

    /// The release page to open ("Release notes", fallback on errors).
    pub fn release_page(&self) -> String {
        self.latest
            .as_ref()
            .map_or_else(super::releases_page, |r| r.page.clone())
    }

    /// Settings' status line.
    pub fn status(&self) -> String {
        let name = crate::config::APP_NAME;
        match &self.phase {
            Phase::Checking { .. } => "Checking for updates…".into(),
            Phase::Downloading { done, total } => match total {
                Some(t) if *t > 0 => format!(
                    "Downloading… {} of {} MB",
                    done / 1_048_576,
                    t.div_ceil(1_048_576)
                ),
                _ => format!("Downloading… {} MB", done / 1_048_576),
            },
            Phase::Verifying => "Verifying the download…".into(),
            Phase::Installing => "Installing…".into(),
            Phase::Installed(v) => format!("{name} {v} is installed. Restart to finish."),
            Phase::InstallFailed(e) => format!("Update failed: {e}"),
            Phase::CheckFailed(e) => format!("Could not check for updates: {e}"),
            _ => match &self.latest {
                Some(r) => format!("{name} {} is available", r.version),
                None if !self.network => "Update checks are off in demo mode".into(),
                None if self.phase == Phase::UpToDate => format!("{name} is up to date"),
                None => String::new(),
            },
        }
    }

    /// Turn the timed checks on or off; returns whether to schedule one.
    pub fn set_auto(&mut self, on: bool) -> bool {
        self.auto = on;
        self.schedule_gen += 1;
        self.auto_enabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::check::Asset;

    fn release(v: Version) -> Release {
        Release {
            version: v,
            tag: format!("v{v}"),
            page: format!("https://github.com/FerroApps/Ferrogram/releases/tag/v{v}"),
            assets: vec![Asset {
                name: "x".into(),
                url: "https://github.com/x".into(),
                size: 1,
            }],
        }
    }

    fn newer() -> Version {
        let c = Version::current();
        Version::new(c.major, c.minor + 1, 0)
    }

    fn install_sh() -> InstallMethod {
        InstallMethod::InstallSh {
            bin_dir: "/b".into(),
            data_dir: "/d".into(),
        }
    }

    fn found(u: &mut Updater, v: Version) {
        assert_eq!(u.begin_check(false), CheckStart::Start);
        u.on_checked(false, Ok(Some(release(v))));
    }

    #[test]
    fn a_newer_release_shows_the_banner_older_or_same_does_not() {
        let mut u = Updater::new(true, None, true, true);
        u.method = Some(install_sh());
        found(&mut u, newer());
        assert_eq!(
            u.banner(),
            Some(Banner::Available {
                version: newer(),
                action: Action::SelfUpdate
            })
        );
        found(&mut u, Version::current());
        assert_eq!(u.banner(), None);
        assert_eq!(u.phase, Phase::UpToDate);
        assert_eq!(u.status(), "Ferrogram is up to date");
    }

    #[test]
    fn dismissal_hides_until_a_newer_version() {
        let mut u = Updater::new(true, None, true, true);
        found(&mut u, newer());
        assert_eq!(u.dismiss(), Some(newer()));
        assert_eq!(u.banner(), None);
        found(&mut u, newer());
        assert_eq!(u.banner(), None, "same version stays dismissed");
        let newest = Version::new(newer().major, newer().minor, 1);
        found(&mut u, newest);
        assert!(matches!(u.banner(), Some(Banner::Available { .. })));
        // A persisted dismissal from an earlier run counts too.
        let mut u = Updater::new(true, Some(newer()), true, true);
        found(&mut u, newer());
        assert_eq!(u.banner(), None);
        assert!(
            u.status().contains("is available"),
            "Settings still says so"
        );
    }

    #[test]
    fn actions_follow_the_install_method() {
        let mut u = Updater::new(true, None, true, true);
        found(&mut u, newer());
        assert_eq!(u.action(), Action::OpenPage, "unknown method");
        assert!(u.start_install().is_none());
        u.method = Some(InstallMethod::Cargo);
        assert_eq!(
            u.action(),
            Action::Cargo(
                "cargo install --git https://github.com/FerroApps/Ferrogram --locked --force"
                    .into()
            )
        );
        assert!(
            u.start_install().is_none(),
            "cargo installs never self-update"
        );
        u.method = Some(install_sh());
        assert!(u.start_install().is_some());
    }

    #[test]
    fn install_flow_progress_cancel_and_restart() {
        let mut u = Updater::new(true, None, true, true);
        u.method = Some(install_sh());
        found(&mut u, newer());
        let (r, cancel) = u.start_install().unwrap();
        assert_eq!(r.version, newer());
        assert!(u.start_install().is_none(), "one at a time");
        assert_eq!(u.begin_check(true), CheckStart::Skip, "no check while busy");
        u.on_progress(Progress::Downloading {
            done: 50,
            total: Some(200),
        });
        assert_eq!(
            u.banner(),
            Some(Banner::Downloading {
                version: newer(),
                percent: Some(25)
            })
        );
        assert_eq!(u.dismiss(), None, "progress cannot be dismissed");
        assert!(u.can_cancel());
        u.cancel();
        assert!(cancel.load(Ordering::Relaxed));
        u.on_installed(Err(UpdateError::Cancelled));
        assert_eq!(u.phase, Phase::Idle);
        assert!(matches!(u.banner(), Some(Banner::Available { .. })));

        let _ = u.start_install().unwrap();
        u.on_progress(Progress::Verifying);
        assert_eq!(u.banner(), Some(Banner::Verifying(newer())));
        u.on_installed(Ok(newer()));
        assert_eq!(u.banner(), Some(Banner::Installed(newer())));
        assert_eq!(u.restart_binary(), Some("/b/ferrogram".into()));
        assert_eq!(u.begin_check(false), CheckStart::Skip);
        // "Later".
        assert_eq!(u.dismiss(), None);
        assert_eq!(u.banner(), None);
        assert!(u.status().contains("Restart to finish"));
    }

    #[test]
    fn failures_show_and_can_be_closed() {
        let mut u = Updater::new(true, None, true, true);
        u.method = Some(install_sh());
        found(&mut u, newer());
        let _ = u.start_install().unwrap();
        u.on_installed(Err(UpdateError::Checksum));
        assert!(matches!(u.banner(), Some(Banner::Failed(m)) if m.contains("checksum")));
        assert_eq!(u.dismiss(), Some(newer()));
        assert_eq!(u.banner(), None);
    }

    #[test]
    fn timed_check_errors_are_silent_manual_ones_are_shown() {
        let mut u = Updater::new(true, None, true, true);
        u.begin_check(false);
        u.on_checked(false, Err(UpdateError::RateLimited));
        assert_eq!(u.phase, Phase::Idle);
        u.begin_check(true);
        u.on_checked(true, Err(UpdateError::RateLimited));
        assert!(matches!(&u.phase, Phase::CheckFailed(m) if m.contains("rate limit")));
        // A late result after something else started is ignored.
        u.phase = Phase::Idle;
        u.on_checked(false, Ok(Some(release(newer()))));
        assert!(u.latest.is_none());
    }

    #[test]
    fn mock_mode_never_uses_the_network() {
        let mut u = Updater::new(true, None, false, false);
        assert!(!u.auto_enabled());
        assert_eq!(u.begin_check(true), CheckStart::Skip);
        assert_eq!(u.status(), "Update checks are off in demo mode");
        u.latest = Some(release(newer()));
        u.method = Some(install_sh());
        assert!(u.start_install().is_none());
    }

    #[test]
    fn auto_toggle_invalidates_pending_checks() {
        let mut u = Updater::new(true, None, true, true);
        assert!(u.auto_enabled());
        let g = u.schedule_gen;
        assert!(!u.set_auto(false));
        assert!(u.schedule_gen > g);
        assert!(u.set_auto(true));
        let mut debug = Updater::new(true, None, false, true);
        assert!(!debug.set_auto(true), "debug builds: manual checks only");
    }
}
