//! Strict semantic versions (`MAJOR.MINOR.PATCH`), enough for comparing a
//! release tag with `CARGO_PKG_VERSION`. Pre-releases are not versions we
//! offer, so they do not parse; build metadata (`+…`) is ignored.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

fn number(s: &str) -> Option<u64> {
    // Semver: digits only, no leading zeros (except "0" itself).
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// `1.2.3` or `1.2.3+build`; `None` for pre-releases (`1.2.3-rc.1`) and
    /// anything that is not a strict semver triple.
    pub fn parse(s: &str) -> Option<Self> {
        let core = s.trim().split_once('+').map_or(s.trim(), |(core, _)| core);
        if core.contains('-') {
            return None;
        }
        let mut parts = core.split('.');
        let v = Self {
            major: number(parts.next()?)?,
            minor: number(parts.next()?)?,
            patch: number(parts.next()?)?,
        };
        parts.next().is_none().then_some(v)
    }

    /// A release tag: `v1.2.3` (the release workflow's form) or `1.2.3`.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::parse(tag.trim().strip_prefix('v').unwrap_or(tag.trim()))
    }

    /// The running build's version.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self::new(0, 0, 0))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| serde::de::Error::custom("not a version"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_triples_only() {
        assert_eq!(Version::parse("0.11.0"), Some(Version::new(0, 11, 0)));
        assert_eq!(Version::parse("1.2.3+build.5"), Some(Version::new(1, 2, 3)));
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.x",
            "a.b.c",
            "1.2.3-rc.1",
            "1..3",
            "-1.2.3",
            "1.2.3 4",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tags_may_carry_a_v() {
        assert_eq!(Version::from_tag("v0.12.0"), Some(Version::new(0, 12, 0)));
        assert_eq!(Version::from_tag("0.12.0"), Some(Version::new(0, 12, 0)));
        assert_eq!(Version::from_tag("v0.12.0-beta"), None);
        assert_eq!(Version::from_tag("vv0.12.0"), None);
    }

    #[test]
    fn compares_numerically_not_lexically() {
        let v = |s| Version::parse(s).unwrap();
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.11.1") > v("0.11.0"));
        assert_eq!(v("0.11.0"), v("0.11.0+x"));
    }

    #[test]
    fn current_matches_the_crate() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn serde_uses_the_string_form() {
        let j = serde_json::to_string(&Version::new(1, 2, 3)).unwrap();
        assert_eq!(j, "\"1.2.3\"");
        assert_eq!(
            serde_json::from_str::<Version>(&j).unwrap(),
            Version::new(1, 2, 3)
        );
        assert!(serde_json::from_str::<Version>("\"1.2\"").is_err());
    }
}
