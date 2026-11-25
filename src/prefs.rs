//! Device preferences for background mode: `settings.ini` next to
//! `config.ini` (never in the SQLite cache, which logout wipes, and never
//! next to the credentials, which `config.ini` handling rewrites).
//!
//! Plain `key = value` lines; unknown keys and malformed values are ignored
//! so an old or hand-edited file can never stop the app from starting.

use std::path::Path;

use crate::update::Version;

/// Global "mute notifications" switch (tray menu).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MuteState {
    #[default]
    Off,
    /// Muted until this Unix time (seconds).
    Until(i64),
    /// Muted until the user turns it back on.
    UntilEnabled,
}

impl MuteState {
    /// Mute for `secs` from `now`.
    pub fn for_secs(now: i64, secs: i64) -> Self {
        Self::Until(now.saturating_add(secs))
    }

    pub fn is_muted(self, now: i64) -> bool {
        match self {
            Self::Off => false,
            Self::Until(t) => t > now,
            Self::UntilEnabled => true,
        }
    }

    /// An expired timer is the same as `Off`; normalise so it is not saved.
    pub fn normalised(self, now: i64) -> Self {
        if self.is_muted(now) { self } else { Self::Off }
    }

    /// Tooltip / settings text while muted: `until 15:30` (local time,
    /// `tz_offset` seconds east of UTC) or `until re-enabled`; `None` when
    /// notifications are not muted.
    pub fn describe(self, now: i64, tz_offset: i32) -> Option<String> {
        match self {
            _ if !self.is_muted(now) => None,
            Self::Until(t) => {
                let local =
                    chrono::DateTime::from_timestamp(t.saturating_add(i64::from(tz_offset)), 0)?;
                Some(format!("until {}", local.format("%H:%M")))
            }
            _ => Some("until re-enabled".into()),
        }
    }

    fn render(self) -> String {
        match self {
            Self::Off => "off".into(),
            Self::Until(t) => format!("until:{t}"),
            Self::UntilEnabled => "until-enabled".into(),
        }
    }

    fn parse(s: &str) -> Self {
        match s.trim() {
            "until-enabled" => Self::UntilEnabled,
            other => other
                .strip_prefix("until:")
                .and_then(|t| t.parse().ok())
                .map_or(Self::Off, Self::Until),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    /// Closing the window keeps the app running in the tray (default on,
    /// like the official clients; only effective while a tray host exists).
    pub close_to_tray: bool,
    /// Start with no window, only the tray icon.
    pub start_minimized: bool,
    pub mute: MuteState,
    /// "Check for updates automatically" (default on).
    pub check_updates: bool,
    /// The update banner the user closed: not shown again for this version
    /// (a newer release shows it again).
    pub update_dismissed: Option<Version>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            start_minimized: false,
            mute: MuteState::Off,
            check_updates: true,
            update_dismissed: None,
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl Prefs {
    pub fn parse(text: &str) -> Self {
        let mut p = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "close_to_tray" => p.close_to_tray = parse_bool(v).unwrap_or(p.close_to_tray),
                "start_minimized" => p.start_minimized = parse_bool(v).unwrap_or(p.start_minimized),
                "notifications_muted" => p.mute = MuteState::parse(v),
                "check_updates" => p.check_updates = parse_bool(v).unwrap_or(p.check_updates),
                "update_dismissed" => p.update_dismissed = Version::parse(v),
                _ => {}
            }
        }
        p
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "close_to_tray = {}\nstart_minimized = {}\nnotifications_muted = {}\ncheck_updates = {}\n",
            self.close_to_tray,
            self.start_minimized,
            self.mute.render(),
            self.check_updates,
        );
        if let Some(v) = self.update_dismissed {
            out.push_str(&format!("update_dismissed = {v}\n"));
        }
        out
    }

    /// Read `path`; a missing or unreadable file means the defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    /// Write atomically (temp file + rename) so a crash cannot truncate it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("ini.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_close_to_tray_like_the_official_apps() {
        let p = Prefs::default();
        assert!(p.close_to_tray && !p.start_minimized);
        assert_eq!(p.mute, MuteState::Off);
        assert_eq!(Prefs::parse(""), p);
    }

    #[test]
    fn round_trip() {
        let p = Prefs {
            close_to_tray: false,
            start_minimized: true,
            mute: MuteState::Until(1_800_000_000),
            check_updates: false,
            update_dismissed: Some(Version::new(0, 12, 3)),
        };
        assert_eq!(Prefs::parse(&p.render()), p);
        let q = Prefs {
            mute: MuteState::UntilEnabled,
            ..p
        };
        assert_eq!(Prefs::parse(&q.render()), q);
    }

    #[test]
    fn dismissed_update_version_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.ini");
        let p = Prefs {
            update_dismissed: Some(Version::new(1, 2, 3)),
            ..Prefs::default()
        };
        p.save(&path).unwrap();
        let back = Prefs::load(&path);
        assert_eq!(back.update_dismissed, Some(Version::new(1, 2, 3)));
        assert!(back.check_updates, "default on");
        // A bad value means "nothing dismissed", never a startup failure.
        let q = Prefs::parse("update_dismissed = 1.2\ncheck_updates = no");
        assert_eq!(q.update_dismissed, None);
        assert!(!q.check_updates);
    }

    #[test]
    fn garbage_is_ignored() {
        let p = Prefs::parse(
            "close_to_tray = maybe\nnope\nstart_minimized=yes\nx=1\nnotifications_muted = until:abc",
        );
        assert!(p.close_to_tray, "bad bool keeps the default");
        assert!(p.start_minimized);
        assert_eq!(p.mute, MuteState::Off);
    }

    #[test]
    fn mute_expires() {
        let m = MuteState::for_secs(1000, 3600);
        assert!(m.is_muted(1000) && m.is_muted(4599));
        assert!(!m.is_muted(4600));
        assert_eq!(m.normalised(5000), MuteState::Off);
        assert!(MuteState::UntilEnabled.is_muted(i64::MAX));
        assert_eq!(
            MuteState::for_secs(i64::MAX, 3600),
            MuteState::Until(i64::MAX)
        );
    }

    #[test]
    fn mute_is_described_in_local_time() {
        // 2026-01-01 12:30 UTC, viewed from UTC+2.
        let t = 1_767_270_600;
        assert_eq!(
            MuteState::Until(t).describe(t - 60, 7200).as_deref(),
            Some("until 14:30")
        );
        assert_eq!(MuteState::Until(t).describe(t + 1, 0), None);
        assert_eq!(MuteState::Off.describe(0, 0), None);
        assert_eq!(
            MuteState::UntilEnabled.describe(0, 0).as_deref(),
            Some("until re-enabled")
        );
    }

    #[test]
    fn save_and_load_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/settings.ini");
        assert_eq!(Prefs::load(&path), Prefs::default());
        let p = Prefs {
            close_to_tray: false,
            ..Prefs::default()
        };
        p.save(&path).unwrap();
        assert_eq!(Prefs::load(&path), p);
        assert!(!path.with_extension("ini.tmp").exists());
    }
}
