//! App-level updater flow with the real `App` in mock mode: the banner
//! appears for a newer release, [x] persists the dismissal, the Settings
//! check works, and nothing reaches the network in mock mode.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrogram::app::{App, Done, Message, Ui};
use ferrogram::config::Config;
use ferrogram::update::install::Progress;
use ferrogram::update::state::{Action, Banner, Phase};
use ferrogram::update::{UpdateError, Version};

fn mock_app() -> App {
    let mut cfg = Config::from_env(&["ferrogram".into(), "--mock".into()]);
    cfg.notifications = false;
    App::boot(cfg).0
}

fn found(app: &mut App, manual: bool) {
    // In mock mode a check never starts by itself; play the result.
    app.updater.phase = Phase::Checking { manual };
    let _ = app.update(Message::Done(Done::UpdateChecked {
        manual,
        result: Ok(Some(ferrogram::mock::mock_release())),
    }));
}

#[test]
fn mock_mode_never_checks() {
    let mut app = mock_app();
    assert!(!app.updater.network && !app.updater.auto_enabled());
    let _ = app.update(Message::Ui(Ui::UpdateCheckNow));
    assert_eq!(app.updater.phase, Phase::Idle);
    let generation = app.updater.schedule_gen;
    let _ = app.update(Message::Ui(Ui::UpdateCheckDue(generation)));
    assert_eq!(app.updater.phase, Phase::Idle);
    assert!(app.updater.banner().is_none());
}

#[test]
fn banner_appears_and_dismissal_is_remembered() {
    let mut app = mock_app();
    found(&mut app, false);
    let v = ferrogram::mock::mock_release().version;
    assert_eq!(
        app.updater.banner(),
        Some(Banner::Available {
            version: v,
            action: Action::SelfUpdate
        })
    );
    let _ = ferrogram::ui::view(&app);
    let _ = app.update(Message::Ui(Ui::UpdateDismiss));
    assert_eq!(
        app.prefs.update_dismissed,
        Some(v),
        "stored in settings.ini"
    );
    assert!(app.updater.banner().is_none());
    // The same version found again stays hidden.
    found(&mut app, false);
    assert!(app.updater.banner().is_none());
    assert!(app.updater.status().contains("is available"));
}

#[test]
fn settings_check_reports_its_result() {
    let mut app = mock_app();
    let _ = app.update(Message::Ui(Ui::OpenSettings));
    // Pretend the network is allowed: the check task is dropped unrun, so
    // no thread or request is ever started.
    app.updater.network = true;
    let _ = app.update(Message::Ui(Ui::UpdateCheckNow));
    assert_eq!(app.updater.phase, Phase::Checking { manual: true });
    let _ = ferrogram::ui::view(&app);
    let _ = app.update(Message::Done(Done::UpdateChecked {
        manual: true,
        result: Ok(None),
    }));
    assert_eq!(app.updater.status(), "Ferrogram is up to date");

    let _ = app.update(Message::Ui(Ui::UpdateCheckNow));
    let _ = app.update(Message::Done(Done::UpdateChecked {
        manual: true,
        result: Err(UpdateError::RateLimited),
    }));
    assert!(app.updater.status().contains("rate limit"));
    let _ = ferrogram::ui::view(&app);
}

#[test]
fn install_progress_cancel_and_restart_prompt() {
    let mut app = mock_app();
    app.updater.network = true;
    found(&mut app, false);
    let _ = app.update(Message::Ui(Ui::UpdateInstall));
    assert!(matches!(app.updater.phase, Phase::Downloading { .. }));
    let _ = app.update(Message::Done(Done::UpdateProgress(Progress::Downloading {
        done: 10,
        total: Some(40),
    })));
    assert!(matches!(
        app.updater.banner(),
        Some(Banner::Downloading {
            percent: Some(25),
            ..
        })
    ));
    let _ = ferrogram::ui::view(&app);
    let _ = app.update(Message::Ui(Ui::UpdateCancel));
    let _ = app.update(Message::Done(Done::UpdateInstalled(Err(
        UpdateError::Cancelled,
    ))));
    assert_eq!(app.updater.phase, Phase::Idle);

    let _ = app.update(Message::Ui(Ui::UpdateInstall));
    let v = ferrogram::mock::mock_release().version;
    let _ = app.update(Message::Done(Done::UpdateInstalled(Ok(v))));
    assert_eq!(app.updater.banner(), Some(Banner::Installed(v)));
    let _ = app.update(Message::Ui(Ui::OpenSettings));
    let _ = ferrogram::ui::view(&app);
    // Restart is a no-op in mock mode (never spawns anything).
    let _ = app.update(Message::Ui(Ui::UpdateRestart));
    // "Later".
    let _ = app.update(Message::Ui(Ui::UpdateDismiss));
    assert!(app.updater.banner().is_none());
    assert_eq!(app.prefs.update_dismissed, None);
}

#[test]
fn auto_check_switch_is_a_preference() {
    let mut app = mock_app();
    assert!(app.prefs.check_updates);
    let _ = app.update(Message::Ui(Ui::SetCheckUpdates(false)));
    assert!(!app.prefs.check_updates && !app.updater.auto);
    let generation = app.updater.schedule_gen;
    let _ = app.update(Message::Ui(Ui::SetCheckUpdates(true)));
    assert!(app.updater.schedule_gen > generation);
    assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
}
