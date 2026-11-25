//! App side of the updater (`crate::update`): schedule the checks, run the
//! blocking work on its own threads, feed results to the pure
//! [`Updater`](crate::update::state::Updater) and restart into a new
//! version.
//!
//! Checks and installs run on plain `std::thread`s, never tokio's blocking
//! pool: a download in progress must never hold up quitting (the runtime
//! waits for blocking-pool tasks when it shuts down).

use std::time::Duration;

use futures::StreamExt;
use iced::Task;

use super::{App, Done, Message, Ui};
use crate::update::check::{self, CheckCache};
use crate::update::http::Endpoint;
use crate::update::install::{self, Layout};
use crate::update::state::{CheckStart, Updater};
use crate::update::{InstallMethod, UpdateError, schedule};

impl App {
    /// The updater for a fresh app (no I/O).
    pub(super) fn new_updater(
        config: &crate::config::Config,
        prefs: &crate::prefs::Prefs,
    ) -> Updater {
        let screenshot = config.screenshot.is_some();
        let mut u = Updater::new(
            prefs.check_updates,
            prefs.update_dismissed,
            schedule::auto_allowed_here(config.mock, screenshot),
            !config.mock || (schedule::force() && !screenshot),
        );
        if config.mock {
            // Demo mode pretends to be an install.sh install (for the
            // screenshots); it never reaches the network or the disk.
            u.method = Some(InstallMethod::InstallSh {
                bin_dir: "/home/demo/.local/bin".into(),
                data_dir: "/home/demo/.local/share".into(),
            });
        }
        u
    }

    /// Startup work: detect the install method off the UI thread and plan
    /// the first check 10–20 s from now. Zero cost until then.
    pub(super) fn boot_updater(&self) -> Task<Message> {
        if self.config.mock && !self.updater.network {
            return Task::none();
        }
        let detect = Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    crate::update::detect::detect(&crate::update::detect::Probe::current())
                })
                .await
                .unwrap_or(InstallMethod::Unmanaged(
                    crate::update::detect::Unmanaged::Other,
                ))
            },
            |m| Message::Done(Done::UpdateMethod(m)),
        );
        let first = if self.updater.auto_enabled() {
            self.schedule_check(schedule::first_delay(schedule::jitter()))
        } else {
            Task::none()
        };
        Task::batch([detect, first])
    }

    fn schedule_check(&self, delay: Duration) -> Task<Message> {
        let generation = self.updater.schedule_gen;
        Task::perform(async move { tokio::time::sleep(delay).await }, move |()| {
            Message::Ui(Ui::UpdateCheckDue(generation))
        })
    }

    fn update_cache_path(&self) -> std::path::PathBuf {
        self.config.cache_dir.join("update-check.json")
    }

    /// One release check on its own thread (started when the task runs,
    /// so a dropped task never touches the network).
    fn run_check(&self, manual: bool) -> Task<Message> {
        let endpoint = Endpoint::from_env();
        let path = self.update_cache_path();
        Task::perform(
            async move {
                let (tx, rx) = futures::channel::oneshot::channel();
                let spawned = std::thread::Builder::new()
                    .name("update-check".into())
                    .spawn(move || {
                        let cache = CheckCache::load(&path);
                        let result = check::check(&endpoint, &cache).map(|(release, fresh)| {
                            if fresh != cache
                                && let Err(e) = fresh.save(&path)
                            {
                                tracing::debug!(error = %e.kind(), "update cache not saved");
                            }
                            release
                        });
                        let _ = tx.send(result);
                    });
                if spawned.is_err() {
                    return Err(UpdateError::Io("no thread".into()));
                }
                rx.await
                    .unwrap_or_else(|_| Err(UpdateError::Network("interrupted".into())))
            },
            move |result| Message::Done(Done::UpdateChecked { manual, result }),
        )
    }

    /// Download and install on its own thread, reporting progress.
    fn run_install(&mut self) -> Task<Message> {
        let Some(InstallMethod::InstallSh { bin_dir, data_dir }) = self.updater.method.clone()
        else {
            return Task::none();
        };
        let Some((release, cancel)) = self.updater.start_install() else {
            return Task::none();
        };
        let layout = Layout { bin_dir, data_dir };
        let endpoint = Endpoint::from_env();
        // The thread starts when iced runs the task.
        let start = async move {
            let (tx, rx) = futures::channel::mpsc::unbounded();
            let fail = tx.clone();
            let spawned = std::thread::Builder::new()
                .name("update-install".into())
                .spawn(move || {
                    let progress_tx = tx.clone();
                    let mut progress = move |p| {
                        let _ = progress_tx.unbounded_send(Done::UpdateProgress(p));
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        install::install(
                            &release,
                            &layout,
                            &endpoint,
                            install::Options::default(),
                            &cancel,
                            &mut progress,
                        )
                    }))
                    .unwrap_or_else(|_| Err(UpdateError::Io("internal error".into())));
                    let _ = tx.unbounded_send(Done::UpdateInstalled(result));
                });
            if spawned.is_err() {
                let _ = fail.unbounded_send(Done::UpdateInstalled(Err(UpdateError::Io(
                    "no thread".into(),
                ))));
            }
            drop(fail);
            rx
        };
        Task::run(futures::stream::once(start).flatten(), Message::Done)
    }

    /// Start the new binary and quit. The instance socket is released
    /// first so the new process becomes the primary instance.
    fn restart_into_update(&mut self) -> Task<Message> {
        let Some(bin) = self.updater.restart_binary() else {
            return Task::none();
        };
        if self.config.mock {
            return Task::none();
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        let args = crate::update::restart::restart_args(&args, !self.window.open);
        let released = crate::instance::release();
        match crate::update::restart::spawn(&bin, &args) {
            Ok(()) => {
                tracing::info!("restarting into the updated binary");
                self.quit()
            }
            Err(e) => {
                tracing::warn!(error = %e.kind(), "could not start the updated binary");
                if let Some(r) = released {
                    crate::instance::acquire(&r.name, crate::instance::Ask::Nothing, r.sink);
                }
                self.toast(format!(
                    "Could not restart ({}); the new version runs on next start",
                    e.kind()
                ))
            }
        }
    }

    fn open_release_page(&self) -> Task<Message> {
        let url = self.updater.release_page();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || crate::desktop::open_url(&url))
                    .await
                    .map_err(|_| crate::error::Error::Cancelled)?
            },
            |r| Message::Done(Done::LinkOpened(r)),
        )
    }

    pub(super) fn update_updates(&mut self, ui: Ui) -> Task<Message> {
        match ui {
            Ui::UpdateCheckDue(generation) => {
                if generation != self.updater.schedule_gen || !self.updater.auto_enabled() {
                    return Task::none();
                }
                // The next one is planned now, so a skipped check (busy)
                // never ends the chain.
                let next = self.schedule_check(schedule::next_delay(schedule::jitter()));
                let check = match self.updater.begin_check(false) {
                    CheckStart::Start => self.run_check(false),
                    CheckStart::Skip => Task::none(),
                };
                Task::batch([next, check])
            }
            Ui::UpdateCheckNow => match self.updater.begin_check(true) {
                CheckStart::Start => self.run_check(true),
                CheckStart::Skip => Task::none(),
            },
            Ui::UpdateInstall => self.run_install(),
            Ui::UpdateCancel => {
                self.updater.cancel();
                Task::none()
            }
            Ui::UpdateDismiss => match self.updater.dismiss() {
                Some(v) => {
                    self.prefs.update_dismissed = Some(v);
                    self.save_prefs()
                }
                None => Task::none(),
            },
            Ui::UpdateRestart => self.restart_into_update(),
            Ui::UpdateReleaseNotes => self.open_release_page(),
            Ui::UpdateCopyCommand => {
                let cmd = crate::update::cargo_install_command();
                Task::batch([
                    iced::clipboard::write(cmd),
                    self.toast("Command copied".into()),
                ])
            }
            Ui::SetCheckUpdates(on) => {
                self.prefs.check_updates = on;
                let save = self.save_prefs();
                if self.updater.set_auto(on) {
                    Task::batch([
                        save,
                        self.schedule_check(schedule::first_delay(schedule::jitter())),
                    ])
                } else {
                    save
                }
            }
            _ => Task::none(),
        }
    }

    pub(super) fn on_update_done(&mut self, done: Done) -> Task<Message> {
        match done {
            Done::UpdateMethod(m) => {
                tracing::info!(method = m.label(), "install method");
                self.updater.method = Some(m);
            }
            Done::UpdateChecked { manual, result } => {
                if self.config.screenshot.is_some() {
                    // Screenshots inject a check result without a check.
                    self.updater.phase = crate::update::state::Phase::Checking { manual };
                }
                self.updater.on_checked(manual, result);
            }
            Done::UpdateProgress(p) => self.updater.on_progress(p),
            Done::UpdateInstalled(r) => self.updater.on_installed(r),
            _ => {}
        }
        Task::none()
    }
}
