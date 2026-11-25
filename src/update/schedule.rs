//! When the automatic checks run: 10–20 s after startup (never on the
//! startup path), then every 6 h ± 30 min while the app runs, tray included.

use std::time::Duration;

pub const FIRST_MIN: Duration = Duration::from_secs(10);
pub const FIRST_MAX: Duration = Duration::from_secs(20);
pub const INTERVAL: Duration = Duration::from_secs(6 * 3600);
pub const INTERVAL_JITTER: Duration = Duration::from_secs(30 * 60);

/// Timed checks are allowed: never in `--mock`, `--screenshot` or debug
/// builds unless `FERRO_UPDATE_FORCE=1`.
pub fn auto_allowed(mock: bool, screenshot: bool, debug_build: bool, force: bool) -> bool {
    if mock || screenshot {
        return force && !screenshot;
    }
    !debug_build || force
}

/// `auto_allowed` for this process.
pub fn auto_allowed_here(mock: bool, screenshot: bool) -> bool {
    auto_allowed(mock, screenshot, cfg!(debug_assertions), force())
}

/// `FERRO_UPDATE_FORCE=1`.
pub fn force() -> bool {
    std::env::var("FERRO_UPDATE_FORCE").is_ok_and(|v| v == "1" || v == "true")
}

fn lerp(min: Duration, max: Duration, unit: f64) -> Duration {
    let unit = if unit.is_finite() {
        unit.clamp(0.0, 1.0)
    } else {
        0.5
    };
    min + (max - min).mul_f64(unit)
}

/// Delay of the first check; `unit` is a random number in `[0, 1)`.
pub fn first_delay(unit: f64) -> Duration {
    lerp(FIRST_MIN, FIRST_MAX, unit)
}

/// Delay until the next periodic check; `unit` is a random number in `[0, 1)`.
pub fn next_delay(unit: f64) -> Duration {
    lerp(INTERVAL - INTERVAL_JITTER, INTERVAL + INTERVAL_JITTER, unit)
}

/// A random `unit` for the functions above.
pub fn jitter() -> f64 {
    rand::random::<f64>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_in_bounds() {
        for u in [0.0, 0.25, 0.5, 0.999, 1.0, -3.0, 7.0, f64::NAN] {
            let f = first_delay(u);
            assert!((FIRST_MIN..=FIRST_MAX).contains(&f), "{u} -> {f:?}");
            let n = next_delay(u);
            assert!(
                (Duration::from_secs(5 * 3600 + 1800)..=Duration::from_secs(6 * 3600 + 1800))
                    .contains(&n),
                "{u} -> {n:?}"
            );
        }
        assert_eq!(first_delay(0.0), Duration::from_secs(10));
        assert_eq!(first_delay(1.0), Duration::from_secs(20));
        for _ in 0..100 {
            let f = first_delay(jitter());
            assert!(f >= FIRST_MIN && f <= FIRST_MAX);
        }
    }

    #[test]
    fn no_timed_checks_in_mock_screenshot_or_debug() {
        // Release build, real mode: on.
        assert!(auto_allowed(false, false, false, false));
        assert!(!auto_allowed(true, false, false, false), "mock");
        assert!(!auto_allowed(false, true, false, false), "screenshot");
        assert!(!auto_allowed(false, false, true, false), "debug build");
        // FERRO_UPDATE_FORCE=1 overrides mock and debug, never screenshots.
        assert!(auto_allowed(false, false, true, true));
        assert!(auto_allowed(true, false, true, true));
        assert!(!auto_allowed(true, true, false, true));
    }
}
