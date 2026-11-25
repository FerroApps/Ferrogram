//! App state + update (the "Elm" side). No I/O happens here directly: every
//! side effect is a `Task` that calls `net`, `store` or `auth`.

pub mod background;
pub mod call;
mod chat_actions;
pub mod chatmenu;
pub mod folders;
pub mod info;
pub mod message;
pub mod phone;
pub mod screenshot;
pub mod state;
mod updates;

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{Id, image, operation};
use iced::{Subscription, Task, Theme, event, keyboard, window};
use tracing::{info, warn};

use background::{Action, Geometry, WindowState};
pub use chatmenu::ChatSearch;
pub use message::{Done, Message, Ui};
use state::{
    ChatList, ComposeMode, Conversation, Lru, PAGE, TypingStep, TypingThrottle, TypingTracker,
};

use crate::auth::secrets::{KeyringStore, MemoryStore, SecretStore};
use crate::auth::vault::Vault;
use crate::auth::{AuthCommand, AuthEvent, AuthState, SignInOutcome};
use crate::config::{Config, ScreenshotTarget};
use crate::error::{AuthError, Error, Redacted};
use crate::mock::{FakeOptions, FakeTelegram, MOCK_NOW};
use crate::model::{
    Chat, ChatId, ConnState, GlobalHit, Media, Message as ChatMessage, MessageId, SendState,
    SharedKind, User,
};
use crate::net::{ApiHandle, Event, UploadKind, backoff};
use crate::prefs::{MuteState, Prefs};
use crate::remote::{MuteRequest, Remote};
use crate::store::Store;
use crate::store::avatars::AvatarDisk;
use crate::tray::{TrayHandle, TrayModel};
use crate::ui::theme::FerroTheme;

pub const HISTORY_ID: &str = "history";
pub const COMPOSER_ID: &str = "composer";
pub const AUTH_INPUT_ID: &str = "auth-input";
pub const CHAT_SEARCH_ID: &str = "chat-search";
pub const FOLDER_TITLE_ID: &str = "folder-title";
/// Image cache budget (architecture §3).
pub const IMAGE_BUDGET: usize = 48 * 1024 * 1024;
/// Photos fetched per open conversation (newest first).
pub const PHOTO_PREFETCH: usize = 20;
/// Decoded-size budget for profile photos (≈100 of them at 160 px).
pub const AVATAR_BUDGET: usize = 10 * 1024 * 1024;
/// Assumed decoded size of one avatar (160 × 160 × 4 bytes).
const AVATAR_COST: usize = 160 * 160 * 4;
/// Chats (from the top of the list) whose avatars are fetched eagerly.
pub const AVATAR_PREFETCH: usize = 80;
/// Dialogs requested per page (first load and each "load more").
pub const DIALOGS_PAGE: usize = 100;
/// Start loading the next page this many px before the list's end.
pub const DIALOGS_LOAD_THRESHOLD: f32 = 300.0;
/// Load the next shared-content page this far (px) before the panel's end.
pub const INFO_LOAD_THRESHOLD: f32 = 400.0;
/// Avatar downloads in flight at once.
pub const AVATAR_CONCURRENCY: usize = 6;

/// First-run form for the Telegram API credentials.
#[derive(Debug, Clone, Default)]
pub struct SetupForm {
    pub api_id: String,
    pub api_hash: crate::auth::SecretInput,
    pub error: Option<String>,
    pub busy: bool,
    /// Opened by the user ("Advanced") rather than forced by missing
    /// credentials; such a form can be cancelled.
    pub advanced: bool,
    /// This build has Ferrogram's own credentials embedded.
    pub has_builtin: bool,
}

impl SetupForm {
    pub fn new(advanced: bool) -> Self {
        Self {
            advanced,
            has_builtin: crate::config::embedded_credentials().is_some(),
            ..Self::default()
        }
    }
}

/// A file waiting for the user to confirm sending (picker or drag & drop).
#[derive(Debug, Clone)]
pub struct PendingFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub kind: UploadKind,
    /// A pasted image we wrote ourselves: deleted once sent or cancelled.
    pub temp: bool,
}

pub struct App {
    pub config: Config,
    pub theme: FerroTheme,
    pub iced_theme: Theme,
    pub api: Option<ApiHandle>,
    /// Bumped whenever a new client is built so the update subscription restarts.
    pub api_gen: u64,
    pub store: Store,
    pub secrets: Arc<dyn SecretStore>,
    pub setup: Option<SetupForm>,
    pub auth: AuthState,
    pub restore_attempt: u32,
    pub me: Option<User>,
    pub chats: ChatList,
    pub selected: Option<ChatId>,
    pub conversation: Option<Conversation>,
    pub sidebar_query: String,
    /// Cached messages matching the sidebar query (all chats).
    pub sidebar_messages: Vec<ChatMessage>,
    /// Server-side (global) search hits for the sidebar query.
    pub sidebar_global: Vec<GlobalHit>,
    pub global_searching: bool,
    pub composer: String,
    pub compose_mode: ComposeMode,
    pub pending_file: Option<PendingFile>,
    /// Text the composer just got from a paste, with the time (ms).
    pub last_paste: Option<(String, i64)>,
    pub emoji_open: bool,
    pub menu_for: Option<MessageId>,
    pub chat_search: Option<ChatSearch>,
    /// The chat header's ⋮ menu, when open.
    pub header_menu: Option<chatmenu::HeaderMenu>,
    /// A destructive action waiting for confirmation.
    pub confirm: Option<chatmenu::Confirm>,
    /// "Select messages" mode of the open chat.
    pub selection: Option<chatmenu::Selection>,
    /// Whether we blocked a user (from `users.getFullUser`), by chat id.
    pub blocked: HashMap<ChatId, bool>,
    pub highlight: Option<MessageId>,
    pub conn: ConnState,
    pub focused: bool,
    pub at_bottom: bool,
    pub photos: Lru<(ChatId, MessageId), image::Handle>,
    pub photo_loading: HashSet<(ChatId, MessageId)>,
    /// Profile photos by chat id, with the photo id they were made from.
    pub avatars: Lru<ChatId, (i64, image::Handle)>,
    pub avatar_loading: HashSet<(ChatId, i64)>,
    /// Photos that failed to download or decode this session (not retried).
    pub avatar_failed: HashSet<(ChatId, i64)>,
    /// `None` in mock mode (no disk writes).
    pub avatar_disk: Option<AvatarDisk>,
    /// Sender names seen in messages, for "Ana is typing…" (bounded).
    pub user_names: HashMap<i64, String>,
    /// The server has no more dialogs after the ones loaded.
    pub dialogs_exhausted: bool,
    pub dialogs_loading: bool,
    /// Bumped on every composer change; a `DraftDue` only fires for the latest.
    pub draft_version: u64,
    pub typing: TypingTracker,
    pub typing_throttle: TypingThrottle,
    pub viewer: Option<(ChatId, MessageId)>,
    pub toast: Option<String>,
    pub main_menu: bool,
    pub now: i64,
    pub tz_offset: i32,
    pub started: Instant,
    pub first_frame_logged: bool,
    // ---- one-to-one calls (app/call.rs, app/phone.rs)
    pub calls: call::Calls,
    pub call_media: Option<phone::CallMedia>,
    pub call_ui: phone::CallUi,
    // ---- background mode (docs/architecture.md section 11.4)
    pub prefs: Prefs,
    /// Window / tray state machine; decides close-to-tray vs quit.
    pub window: WindowState,
    /// The one window while it is open (the app is an `iced::daemon`).
    pub window_id: Option<window::Id>,
    /// Last size and position, restored when the window is shown again.
    pub geometry: Geometry,
    pub tray: Option<TrayHandle>,
    /// What the tray last showed; only changes are sent to the panel.
    pub tray_sent: Option<TrayModel>,
    pub settings_open: bool,
    /// Chat folders: list, selection and open menus.
    pub folders: folders::FolderState,
    /// The folder editor dialog, when open.
    pub folder_editor: Option<folders::FolderEditor>,
    /// The right-hand info panel of the open chat, when shown.
    pub info: Option<info::InfoPanel>,
    /// Previews of the shared photos/videos/GIFs (bounded, encoded bytes).
    pub thumbs: Lru<(ChatId, MessageId), image::Handle>,
    pub thumb_q: info::ThumbQueue,
    /// "Launch at login" as last read from / written to disk.
    pub autostart: bool,
    /// The window's scale factor (1.0 until the window reports it).
    pub scale: f32,
    /// In-app updates (banner, Settings section; `crate::update`).
    pub updater: crate::update::state::Updater,
}

/// Requests from outside the window (see `remote`).
fn remote_requests() -> impl futures::Stream<Item = Message> {
    use futures::StreamExt;
    crate::remote::stream().map(|r| Message::Ui(Ui::Remote(r)))
}

/// Hashable wrapper so the update stream can be a `Subscription::run_with`.
/// The generation makes iced start a fresh stream for every new client.
struct EventsSource(ApiHandle, u64);

impl Hash for EventsSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "ferrogram-events".hash(state);
        self.1.hash(state);
    }
}

fn events_stream(src: &EventsSource) -> impl futures::Stream<Item = Message> + use<> {
    use futures::StreamExt;
    src.0.events().map(Message::Net)
}

/// Errors after which `restore()` should simply be retried (we may well
/// have a valid session and just be offline).
pub fn is_transient(e: &Error) -> bool {
    matches!(e, Error::Net(_) | Error::Cancelled)
}

impl App {
    pub fn boot(config: Config) -> (Self, Task<Message>) {
        let theme = if config.light_theme {
            FerroTheme::day()
        } else {
            FerroTheme::night()
        };
        let tz_offset = chrono::Local::now().offset().local_minus_utc();

        let (store, secrets): (Store, Arc<dyn SecretStore>) = if config.mock {
            (open_memory_store(), Arc::new(MemoryStore::new()))
        } else {
            let store = Store::open(&config.cache_db_path()).unwrap_or_else(|e| {
                warn!(error = %e, "cache unavailable, using in-memory store");
                open_memory_store()
            });
            (store, Arc::new(KeyringStore))
        };

        // Device preferences (mock mode never touches the disk).
        let prefs = if config.mock {
            Prefs::default()
        } else {
            Prefs::load(&config.prefs_path())
        };
        let autostart = !config.mock && crate::autostart::system_is_enabled();
        let geometry = if config.screenshot.is_some() {
            Geometry {
                size: (1280.0, 800.0),
                position: None,
            }
        } else {
            Geometry::default()
        };

        // First frame comes from the cache, never from the network.
        let cached = store.load_chats().unwrap_or_default();
        let cached_defaults = store.load_notify_defaults().unwrap_or_default();
        let mut cached_folders = folders::FolderState::default();
        cached_folders.set_list(store.load_folders().unwrap_or_default());
        let mut app = Self {
            iced_theme: theme.iced(),
            theme,
            api: None,
            api_gen: 0,
            store,
            secrets,
            setup: None,
            auth: AuthState::Restoring,
            restore_attempt: 0,
            me: None,
            chats: {
                let mut list = ChatList::new(cached);
                list.set_defaults(cached_defaults, chrono::Utc::now().timestamp());
                list
            },
            selected: None,
            conversation: None,
            sidebar_query: String::new(),
            sidebar_messages: Vec::new(),
            sidebar_global: Vec::new(),
            global_searching: false,
            composer: String::new(),
            compose_mode: ComposeMode::Normal,
            pending_file: None,
            last_paste: None,
            emoji_open: false,
            menu_for: None,
            chat_search: None,
            header_menu: None,
            confirm: None,
            selection: None,
            blocked: HashMap::new(),
            highlight: None,
            conn: ConnState::Connecting,
            focused: true,
            at_bottom: true,
            photos: Lru::new(IMAGE_BUDGET),
            photo_loading: HashSet::new(),
            avatars: Lru::new(AVATAR_BUDGET),
            avatar_loading: HashSet::new(),
            avatar_failed: HashSet::new(),
            avatar_disk: (!config.mock).then(|| AvatarDisk::new(config.avatar_dir())),
            user_names: HashMap::new(),
            dialogs_exhausted: false,
            dialogs_loading: false,
            draft_version: 0,
            typing: TypingTracker::default(),
            typing_throttle: TypingThrottle::default(),
            viewer: None,
            toast: None,
            main_menu: false,
            now: if config.mock {
                MOCK_NOW
            } else {
                chrono::Utc::now().timestamp()
            },
            tz_offset: if config.mock { 0 } else { tz_offset },
            started: Instant::now(),
            first_frame_logged: !config.trace_startup,
            calls: call::Calls::default(),
            call_media: None,
            call_ui: phone::CallUi::default(),
            prefs,
            window: WindowState::new(prefs.close_to_tray),
            window_id: None,
            geometry,
            tray: None,
            tray_sent: None,
            settings_open: false,
            folders: cached_folders,
            folder_editor: None,
            info: None,
            thumbs: Lru::new(info::THUMB_BUDGET),
            thumb_q: info::ThumbQueue::default(),
            autostart,
            scale: 1.0,
            updater: Self::new_updater(&config, &prefs),
            config,
        };

        let task = if app.config.mock {
            let logged_in = !matches!(
                app.config.screenshot.as_ref().map(|s| s.0),
                Some(
                    ScreenshotTarget::LoginPhone
                        | ScreenshotTarget::LoginCode
                        | ScreenshotTarget::LoginPassword
                        | ScreenshotTarget::ApiSetup
                )
            ) && std::env::var("FERROGRAM_MOCK_LOGGED_OUT").is_err();
            let fake = FakeTelegram::new(FakeOptions {
                logged_in,
                live_simulation: app.config.screenshot.is_none(),
                latency: Duration::from_millis(if app.config.screenshot.is_some() {
                    0
                } else {
                    150
                }),
                extra_chats: std::env::var("FERROGRAM_MOCK_EXTRA_CHATS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
            });
            Task::done(Message::Done(Done::ApiReady(Ok(ApiHandle(fake)))))
        } else if app.config.api_setup {
            // `--api-setup`: ask for the user's own credentials first.
            app.setup = Some(SetupForm::new(true));
            Task::none()
        } else {
            app.start_real_api()
        };
        let script = screenshot::script(&app.config);

        // Window and tray. A screenshot run has no tray and always a window.
        let screenshot = app.config.screenshot.is_some();
        let minimized = !screenshot && (app.config.minimized || app.prefs.start_minimized);
        let open = match app.window.start(minimized) {
            Action::Open => app.open_window(false),
            _ => Task::none(),
        };
        let tray = if screenshot {
            // No panel to talk to: behave exactly like a plain window.
            app.window.tray_ready(false);
            Task::none()
        } else {
            app.start_tray(minimized)
        };

        // Pasted images left behind by a crash (they are deleted on send/cancel).
        if !app.config.mock {
            let dir = app.config.paste_dir();
            std::thread::spawn(move || {
                let _ = std::fs::remove_dir_all(dir);
            });
        }
        // Cached avatars from disk are on screen with the first frame.
        let avatars = app.fetch_avatars();
        let updates = app.boot_updater();
        (
            app,
            Task::batch([open, tray, task, script, avatars, updates]),
        )
    }

    // ------------------------------------------------- window and tray

    fn window_settings(&self) -> window::Settings {
        let g = self.geometry;
        window::Settings {
            size: iced::Size::new(g.size.0, g.size.1),
            position: g.position.map_or(window::Position::Default, |(x, y)| {
                window::Position::Specific(iced::Point::new(x, y))
            }),
            min_size: Some(iced::Size::new(
                background::MIN_SIZE.0,
                background::MIN_SIZE.1,
            )),
            resizable: self.config.screenshot.is_none(),
            // The close button is a request we answer ourselves: to the
            // tray when there is one, otherwise by quitting.
            exit_on_close_request: false,
            #[cfg(target_os = "linux")]
            platform_specific: window::settings::PlatformSpecific {
                application_id: crate::config::APP_ID.into(),
                ..Default::default()
            },
            ..window::Settings::default()
        }
    }

    /// Open the window; `raise` also asks the compositor to focus it (a
    /// window shown from the tray, as opposed to the first one at startup).
    fn open_window(&mut self, raise: bool) -> Task<Message> {
        let (id, opened) = window::open(self.window_settings());
        self.window_id = Some(id);
        opened.then(move |id| {
            let opened = Task::done(Message::Done(Done::WindowOpened(id)));
            if raise {
                Task::batch([window::gain_focus(id), opened])
            } else {
                opened
            }
        })
    }

    /// Close the window but keep running. The window (and with it the
    /// renderer surfaces) is freed; so are the decoded images, which are
    /// reloaded from the disk cache when the window comes back.
    fn close_window(&mut self) -> Task<Message> {
        let Some(id) = self.window_id.take() else {
            return Task::none();
        };
        self.focused = false;
        self.viewer = None;
        self.menu_for = None;
        self.emoji_open = false;
        self.main_menu = false;
        self.settings_open = false;
        self.photos = Lru::new(IMAGE_BUDGET);
        self.photo_loading.clear();
        self.avatars = Lru::new(AVATAR_BUDGET);
        self.avatar_loading.clear();
        // The window and renderer are dropped when the runtime handles the
        // close, a moment after this returns: trim the heap after that.
        let trim = Task::perform(
            async {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let _ = tokio::task::spawn_blocking(crate::memory::release_to_os).await;
            },
            |()| Message::Done(Done::Call(Ok(()))),
        );
        Task::batch([window::close(id), trim])
    }

    /// Carry out what the window state machine decided.
    fn apply_window(&mut self, action: Action) -> Task<Message> {
        match action {
            Action::None => Task::none(),
            Action::Open => {
                let open = self.open_window(true);
                // Images were dropped while hidden: load what is on screen.
                Task::batch([open, self.fetch_avatars(), self.fetch_photos()])
            }
            Action::CloseWindow => self.close_window(),
            Action::Focus => self.window_id.map_or_else(Task::none, |id| {
                Task::batch([window::minimize(id, false), window::gain_focus(id)])
            }),
            Action::Exit => self.quit(),
        }
    }

    /// Stop the tray service, then end the process.
    fn quit(&mut self) -> Task<Message> {
        // A download in progress stops at its next chunk (its thread never
        // holds up the exit).
        self.updater.cancel();
        match self.tray.take() {
            Some(tray) => {
                Task::future(async move { tray.shutdown().await }).then(|()| iced::exit())
            }
            None => iced::exit(),
        }
    }

    fn start_tray(&self, minimized: bool) -> Task<Message> {
        // At login the panel may start after us: a minimized start keeps
        // trying for a while before the window is shown instead.
        let attempts = if minimized { 15 } else { 3 };
        let model = self.tray_model();
        let sink: crate::tray::Sink = Arc::new(crate::remote::send);
        Task::perform(
            crate::tray::spawn(model, sink, attempts, Duration::from_secs(1)),
            |r| Message::Done(Done::TrayReady(r)),
        )
    }

    fn real_now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    pub fn muted(&self) -> bool {
        self.prefs.mute.is_muted(Self::real_now())
    }

    /// What the tray should show right now.
    pub fn tray_model(&self) -> TrayModel {
        TrayModel {
            unread: if self.auth.is_signed_in() {
                self.chats.total_unread()
            } else {
                0
            },
            window_open: self.window.open,
            ringing: self.calls.is_ringing_in(),
            muted: self.prefs.mute.describe(Self::real_now(), self.tz_offset),
            start_minimized: self.prefs.start_minimized,
        }
    }

    /// Push the tray model to the panel when it changed (called after every
    /// update, so unread counts and state always follow the app).
    fn sync_tray(&mut self) -> Option<Task<Message>> {
        let tray = self.tray.clone()?;
        let model = self.tray_model();
        if self.tray_sent.as_ref() == Some(&model) {
            return None;
        }
        self.tray_sent = Some(model.clone());
        Some(Task::future(async move { tray.set_model(model).await }).discard())
    }

    fn save_prefs(&self) -> Task<Message> {
        if self.config.mock {
            return Task::none();
        }
        let (prefs, path) = (self.prefs, self.config.prefs_path());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || prefs.save(&path))
                    .await
                    .map_err(|_| Error::Cancelled)?
                    .map_err(|e| Error::Io(format!("settings: {}", e.kind())))
            },
            |r| Message::Done(Done::Call(r)),
        )
    }

    fn on_remote(&mut self, request: Remote) -> Task<Message> {
        match request {
            Remote::Mute(m) => {
                let now = Self::real_now();
                self.prefs.mute = match m {
                    MuteRequest::OneHour => MuteState::for_secs(now, 3600),
                    MuteRequest::UntilEnabled => MuteState::UntilEnabled,
                    MuteRequest::Off => MuteState::Off,
                };
                self.save_prefs()
            }
            Remote::ToggleStartMinimized => {
                self.update_ui(Ui::SetStartMinimized(!self.prefs.start_minimized))
            }
            other => {
                let action = self.window.handle(other);
                let mut tasks = vec![self.apply_window(action)];
                if let Remote::Show { chat: Some(chat) } = other
                    && self.chats.get(chat).is_some()
                {
                    tasks.push(self.update_ui(Ui::SelectChat(chat)));
                }
                Task::batch(tasks)
            }
        }
    }

    /// Build a brand-new grammers client (boot, and after logout/revoke so
    /// no auth key, update receiver or event channel is reused).
    fn start_real_api(&self) -> Task<Message> {
        let vault = Vault::new(self.secrets.clone(), self.config.session_blob_path());
        let config = self.config.clone();
        let secrets = self.secrets.clone();
        Task::perform(
            async move {
                let creds = tokio::task::spawn_blocking(move || {
                    config.load_api_credentials(secrets.as_ref())
                })
                .await
                .map_err(|_| Error::Cancelled)??;
                let api = crate::net::mtproto::GrammersApi::start(creds, vault).await?;
                Ok(ApiHandle(Arc::new(crate::net::mtproto::Grammers(api))))
            },
            |r| Message::Done(Done::ApiReady(r)),
        )
    }

    pub fn title(&self) -> String {
        let unread = self.chats.total_unread();
        if unread > 0 && self.auth.is_signed_in() {
            format!("Ferrogram ({unread})")
        } else {
            "Ferrogram".into()
        }
    }

    pub fn theme(&self) -> Theme {
        self.iced_theme.clone()
    }

    /// Show the main window (not the login screen): signed in, or still
    /// restoring a session while offline with a cached chat list.
    pub fn shows_main(&self) -> bool {
        self.auth.is_signed_in()
            || (self.auth == AuthState::Restoring
                && self.setup.is_none()
                && !self.chats.all().is_empty())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![event::listen_with(|ev, _status, _id| match ev {
            iced::Event::Window(window::Event::Focused) => Some(Message::Ui(Ui::Focus(true))),
            iced::Event::Window(window::Event::Unfocused) => Some(Message::Ui(Ui::Focus(false))),
            iced::Event::Window(window::Event::FileDropped(p)) => {
                Some(Message::Ui(Ui::FileDropped(p)))
            }
            iced::Event::Window(window::Event::CloseRequested) => {
                Some(Message::Ui(Ui::CloseRequested))
            }
            iced::Event::Window(window::Event::Resized(size)) => {
                Some(Message::Ui(Ui::WindowResized(size)))
            }
            iced::Event::Window(window::Event::Moved(p)) => Some(Message::Ui(Ui::WindowMoved(p))),
            iced::Event::Window(window::Event::Rescaled(f)) => Some(Message::Ui(Ui::Rescaled(f))),
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => Some(Message::Ui(Ui::Escape)),
            // Up / Down walk the in-chat search hits (ignored when no search
            // is open, and when a widget used the key).
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::ArrowUp),
                ..
            }) => Some(Message::Ui(Ui::ChatSearchStep(chatmenu::Dir::Older))),
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::ArrowDown),
                ..
            }) => Some(Message::Ui(Ui::ChatSearchStep(chatmenu::Dir::Newer))),
            // Ctrl+V (any layout: the physical key). Delivered even when the
            // composer captured it for the text paste.
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyV),
                modifiers,
                ..
            }) if modifiers.command() && !modifiers.shift() && !modifiers.alt() => {
                Some(Message::Ui(Ui::Paste))
            }
            _ => None,
        })];
        // Tray menu, second launch and notification clicks.
        subs.push(Subscription::run(remote_requests));
        if let (Some(api), true) = (&self.api, self.auth.is_signed_in()) {
            subs.push(Subscription::run_with(
                EventsSource(api.clone(), self.api_gen),
                events_stream,
            ));
        }
        if !self.typing.is_empty() {
            subs.push(
                iced::time::every(Duration::from_secs(1)).map(|_| Message::Ui(Ui::TypingTick)),
            );
        }
        if !self.config.mock {
            subs.push(iced::time::every(Duration::from_secs(30)).map(|_| Message::Ui(Ui::Tick)));
        }
        if !self.first_frame_logged {
            subs.push(window::frames().map(|_| Message::Ui(Ui::Tick)));
        }
        self.call_subscriptions(&mut subs);
        Subscription::batch(subs)
    }

    // ------------------------------------------------------------- update

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = match message {
            Message::Ui(ui) => self.update_ui(ui),
            Message::Net(ev) => self.on_event(ev),
            Message::Done(done) => self.on_done(done),
        };
        match self.sync_tray() {
            Some(sync) => Task::batch([task, sync]),
            None => task,
        }
    }

    fn auth(&mut self, ev: AuthEvent) -> Task<Message> {
        let Some(cmd) = crate::auth::reduce(&mut self.auth, ev) else {
            return Task::none();
        };
        let Some(api) = self.api.clone() else {
            // No client yet (still starting, or it failed): fail the step so
            // the form never stays busy forever.
            let err = Error::Net("not connected".into());
            let ev = match cmd {
                AuthCommand::RequestCode(_) => AuthEvent::CodeRequested(Err(err)),
                AuthCommand::SignIn { .. } | AuthCommand::CheckPassword(_) => {
                    AuthEvent::SignIn(Err(err))
                }
                AuthCommand::PersistSession => return Task::none(),
            };
            crate::auth::reduce(&mut self.auth, ev);
            return Task::none();
        };
        match cmd {
            AuthCommand::RequestCode(phone) => {
                Task::perform(async move { api.request_code(phone).await }, |r| {
                    Message::Done(Done::CodeRequested(r))
                })
            }
            AuthCommand::SignIn { code } => {
                Task::perform(async move { api.sign_in(code).await }, |r| {
                    Message::Done(Done::SignIn(r))
                })
            }
            AuthCommand::CheckPassword(pw) => {
                Task::perform(async move { api.check_password(pw).await }, |r| {
                    Message::Done(Done::SignIn(r))
                })
            }
            AuthCommand::PersistSession => {
                Task::perform(async move { api.persist_session().await }, |r| {
                    Message::Done(Done::Persisted(r))
                })
            }
        }
    }

    fn on_signed_in(&mut self, user: Option<User>) -> Task<Message> {
        let mut tasks = vec![
            self.fetch_dialogs(),
            self.fetch_folders(),
            self.fetch_notify_defaults(),
        ];
        if let Some(u) = &user {
            let id = u.id.to_string();
            tasks.push(self.store_bg(move |s| s.set_kv("self_id", &id)));
            self.me = user;
        }
        self.conn = ConnState::Connected;
        self.restore_attempt = 0;
        if self.config.screenshot.is_none() {
            tasks.push(operation::focus(Id::new(COMPOSER_ID)));
        }
        Task::batch(tasks)
    }

    // ------------------------------------------------------ info panel

    /// Show the info panel for the open chat (or re-target it).
    fn open_info(&mut self) -> Task<Message> {
        let Some(chat) = self.selected else {
            return Task::none();
        };
        self.info = Some(info::InfoPanel::new(chat));
        self.reset_thumbs();
        Task::batch([
            self.fetch_peer_info(chat),
            self.load_shared(SharedKind::Media),
        ])
    }

    /// Hide the panel and drop its thumbnails. `true` when it was open.
    fn close_info(&mut self) -> bool {
        if self.info.take().is_none() {
            return false;
        }
        self.reset_thumbs();
        true
    }

    fn reset_thumbs(&mut self) {
        self.thumbs = Lru::new(info::THUMB_BUDGET);
        self.thumb_q.clear();
    }

    fn fetch_peer_info(&self, chat: ChatId) -> Task<Message> {
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        Task::perform(async move { api.peer_info(chat).await }, move |result| {
            Message::Done(Done::PeerInfo { chat, result })
        })
    }

    /// Request the next page of a tab, if one is due.
    fn load_shared(&mut self, kind: SharedKind) -> Task<Message> {
        let Some(panel) = &mut self.info else {
            return Task::none();
        };
        let chat = panel.chat;
        let Some(offset) = panel.tab_ref(kind).map_or(Some(0), info::SharedTab::want) else {
            return Task::none();
        };
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        panel.tab_mut(kind).start(offset);
        Task::perform(
            async move { api.shared_media(chat, kind, offset, info::PAGE).await },
            move |result| {
                Message::Done(Done::SharedPage {
                    chat,
                    kind,
                    offset,
                    result,
                })
            },
        )
    }

    /// Queue the previews the visible grid rows (and a margin) still need,
    /// dropping the ones scrolled away from, and start downloads.
    fn request_thumbs(&mut self) -> Task<Message> {
        let Some(panel) = &self.info else {
            return Task::none();
        };
        let chat = panel.chat;
        let wanted = panel.wanted_thumbs();
        let thumbs = &self.thumbs;
        self.thumb_q
            .set_wanted(wanted.into_iter().map(|id| (chat, id)), |k| {
                thumbs.peek(k).is_some()
            });
        self.pump_thumbs()
    }

    /// Start the queued thumbnail downloads that fit the concurrency limit.
    fn pump_thumbs(&mut self) -> Task<Message> {
        // Nothing is drawn while the window is closed in the tray.
        if self.window_id.is_none() {
            return Task::none();
        }
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        let tasks: Vec<_> = self
            .thumb_q
            .take_ready()
            .into_iter()
            .map(|key| {
                let api = api.clone();
                Task::perform(
                    async move { api.download_thumb(key.0, key.1).await },
                    move |result| Message::Done(Done::Thumb { key, result }),
                )
            })
            .collect();
        Task::batch(tasks)
    }

    /// Load a photo for the viewer when it is not cached.
    fn ensure_photo(&mut self, key: (ChatId, MessageId)) -> Task<Message> {
        if key.1 <= 0 || self.photos.peek(&key).is_some() || !self.photo_loading.insert(key) {
            return Task::none();
        }
        let Some(api) = self.api.clone() else {
            self.photo_loading.remove(&key);
            return Task::none();
        };
        Task::perform(
            async move { api.download_photo(key.0, key.1).await },
            move |result| Message::Done(Done::Photo { key, result }),
        )
    }

    // ------------------------------------------------------- folders

    fn fetch_folders(&self) -> Task<Message> {
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        Task::perform(async move { api.dialog_filters().await }, |r| {
            Message::Done(Done::Folders(r))
        })
    }

    /// Cache the folder list for the next first frame.
    fn persist_folders(&self) -> Task<Message> {
        let list = self.folders.list.clone();
        self.store_bg(move |s| s.save_folders(&list))
    }

    /// Apply a folder change at once and tell the server; the list is put
    /// back if the server refuses.
    fn save_folder_optimistic(&mut self, folder: crate::model::Folder) -> Task<Message> {
        let before = self.folders.list.clone();
        self.folders.upsert(folder.clone());
        let persist = self.persist_folders();
        let Some(api) = self.api.clone() else {
            return persist;
        };
        Task::batch([
            persist,
            Task::perform(
                async move { api.save_dialog_filter(folder).await },
                move |result| Message::Done(Done::FolderOp { before, result }),
            ),
        ])
    }

    /// Folders are evaluated on the chats we have loaded: while a folder
    /// shows only a few and the account has more dialogs, page them in.
    fn fill_folder(&mut self) -> Task<Message> {
        const WANT: usize = 15;
        let Some(f) = self.folders.current() else {
            return Task::none();
        };
        if folders::FolderState::chats_in(f, self.chats.all()).len() >= WANT {
            return Task::none();
        }
        self.load_more_dialogs()
    }

    /// Chats shown in the list: the selected folder's, or all.
    pub fn visible_chats(&self) -> Vec<&Chat> {
        match self.folders.current() {
            Some(f) => folders::FolderState::chats_in(f, self.chats.all()),
            None => self.chats.all().iter().collect(),
        }
    }

    fn fetch_dialogs(&self) -> Task<Message> {
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        Task::perform(async move { api.dialogs(DIALOGS_PAGE).await }, |r| {
            Message::Done(Done::Dialogs(r))
        })
    }

    /// Load the next page of dialogs once the user nears the list's end.
    fn load_more_dialogs(&mut self) -> Task<Message> {
        if self.dialogs_exhausted || self.dialogs_loading || !self.auth.is_signed_in() {
            return Task::none();
        }
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        self.dialogs_loading = true;
        Task::perform(async move { api.more_dialogs(DIALOGS_PAGE).await }, |r| {
            Message::Done(Done::MoreDialogs(r))
        })
    }

    fn api_call<F>(&self, f: F) -> Task<Message>
    where
        F: FnOnce(ApiHandle) -> futures::future::BoxFuture<'static, crate::error::Result<()>>
            + Send
            + 'static,
    {
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        Task::perform(f(api), |r| Message::Done(Done::Call(r)))
    }

    /// A background call whose failure is only logged (typing indicators and
    /// draft sync must never pop up a toast).
    fn api_call_quiet<F>(&self, what: &'static str, f: F) -> Task<Message>
    where
        F: FnOnce(ApiHandle) -> futures::future::BoxFuture<'static, crate::error::Result<()>>
            + Send
            + 'static,
    {
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        Task::perform(f(api), move |r| {
            if let Err(e) = r {
                warn!(error = %e, what, "background call failed");
            }
            Message::Done(Done::Call(Ok(())))
        })
    }

    /// An optimistic call that restores `old` if it fails.
    fn api_call_or_rollback<F>(&self, old: ChatMessage, f: F) -> Task<Message>
    where
        F: FnOnce(ApiHandle) -> futures::future::BoxFuture<'static, crate::error::Result<()>>
            + Send
            + 'static,
    {
        let Some(api) = self.api.clone() else {
            return Task::done(Message::Done(Done::Rollback {
                old,
                error: Error::Net("not connected".into()),
            }));
        };
        Task::perform(f(api), move |r| match r {
            Ok(()) => Message::Done(Done::Call(Ok(()))),
            Err(error) => Message::Done(Done::Rollback {
                old: old.clone(),
                error,
            }),
        })
    }

    fn store_bg<F>(&self, f: F) -> Task<Message>
    where
        F: FnOnce(&Store) -> crate::error::Result<()> + Send + 'static,
    {
        let store = self.store.clone();
        Task::perform(async move { store.call(f).await }, |r| {
            Message::Done(Done::Call(r))
        })
    }

    fn open_chat(&mut self, chat: ChatId) -> Task<Message> {
        if self.selected == Some(chat) && self.conversation.as_ref().is_some_and(|c| !c.detached) {
            return Task::none();
        }
        let switch = self.switch_to(chat, Conversation::new(chat));
        Task::batch([switch, self.load_newest(chat)])
    }

    /// Open `chat` on a window around message `id` (search hit, link). The
    /// target's page is fetched from the network; the newest page is not
    /// loaded until the user jumps back to the latest messages.
    fn open_chat_at(&mut self, chat: ChatId, id: MessageId) -> Task<Message> {
        self.highlight = Some(id);
        if self.selected == Some(chat)
            && let Some(conv) = &self.conversation
            && let Some(i) = conv.index_of(id)
        {
            let y = state::jump_offset(i, conv.messages().len());
            return operation::snap_to(
                Id::new(HISTORY_ID),
                operation::RelativeOffset { x: 0.0, y },
            );
        }
        if id <= 0 {
            return self.open_chat(chat);
        }
        let switch = self.switch_to(chat, Conversation::new_at(chat, id));
        self.highlight = Some(id);
        let before = id.saturating_add(state::JUMP_NEWER);
        Task::batch([switch, self.net_history(chat, Some(before))])
    }

    /// Reload the newest messages after browsing an older window.
    fn reattach(&mut self) -> Task<Message> {
        let Some(chat) = self.selected else {
            return Task::none();
        };
        self.conversation = Some(Conversation::new(chat));
        self.highlight = None;
        self.at_bottom = true;
        self.chats.mark_read(chat);
        self.load_newest(chat)
    }

    /// Load the newest page of `chat` (cache first, then network) and mark
    /// it read.
    fn load_newest(&self, chat: ChatId) -> Task<Message> {
        let store = self.store.clone();
        // The newest cached page is shown immediately; the network page that
        // is always requested alongside replaces/extends it.
        let cache = Task::perform(
            async move { store.call(move |s| s.load_history(chat, None, PAGE)).await },
            move |result| {
                Message::Done(Done::CacheHistory {
                    chat,
                    before: None,
                    complete: false,
                    result,
                })
            },
        );
        let net = self.net_history(chat, None);
        let read = self.api_call(move |api| Box::pin(async move { api.mark_read(chat).await }));
        let unread = self.store_bg(move |s| s.set_unread(chat, 0, 0));
        Task::batch([cache, net, read, unread])
    }

    /// Make `conversation` the open one and reset per-chat UI state.
    fn switch_to(&mut self, chat: ChatId, conversation: Conversation) -> Task<Message> {
        let stop_typing = match self.typing_throttle.reset() {
            Some(old) => self.api_call_quiet("typing", move |api| {
                Box::pin(async move { api.set_typing(old, false).await })
            }),
            None => Task::none(),
        };
        // Leaving a chat saves what was typed there as its draft.
        let save_draft = match self.selected {
            Some(old) if old != chat => self.sync_draft(old),
            _ => Task::none(),
        };
        // A search keeps running while it jumps around the same chat.
        let same_chat = self.selected == Some(chat);
        self.selected = Some(chat);
        self.conversation = Some(conversation);
        self.compose_mode = ComposeMode::Normal;
        self.drop_pending();
        self.menu_for = None;
        if !same_chat {
            self.chat_search = None;
        }
        self.header_menu = None;
        self.confirm = None;
        self.selection = None;
        self.highlight = None;
        self.emoji_open = false;
        self.at_bottom = true;
        if !self.conversation.as_ref().is_some_and(|c| c.detached) {
            self.chats.mark_read(chat);
        }
        self.restore_draft(chat);
        // An open info panel follows the chat.
        let info = if self.info.as_ref().is_some_and(|p| p.chat != chat) {
            self.open_info()
        } else {
            Task::none()
        };
        Task::batch([
            info,
            stop_typing,
            save_draft,
            self.fetch_avatars(),
            operation::focus(Id::new(COMPOSER_ID)),
        ])
    }

    fn net_history(&self, chat: ChatId, before: Option<MessageId>) -> Task<Message> {
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { api.history(chat, before, PAGE).await },
            move |result| {
                Message::Done(Done::NetHistory {
                    chat,
                    before,
                    result,
                })
            },
        )
    }

    fn load_older(&mut self) -> Task<Message> {
        let Some(conv) = &mut self.conversation else {
            return Task::none();
        };
        if conv.loading_older || conv.reached_start {
            return Task::none();
        }
        let Some(before) = conv.oldest_id() else {
            return Task::none();
        };
        conv.loading_older = true;
        let chat = conv.chat_id;
        let store = self.store.clone();
        Task::perform(
            async move {
                store
                    .call(move |s| s.load_older_cached(chat, before, PAGE))
                    .await
            },
            move |result| {
                let (result, complete) = match result {
                    Ok((msgs, complete)) => (Ok(msgs), complete),
                    Err(e) => (Err(e), false),
                };
                Message::Done(Done::CacheHistory {
                    chat,
                    before: Some(before),
                    complete,
                    result,
                })
            },
        )
    }

    /// Request (thumbnail) downloads for photos in the open conversation.
    fn fetch_photos(&mut self) -> Task<Message> {
        // Nothing is drawn while the window is closed in the tray.
        if self.window_id.is_none() {
            return Task::none();
        }
        let Some(conv) = &self.conversation else {
            return Task::none();
        };
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        let photos = conv
            .messages()
            .iter()
            .rev()
            .filter(|m| m.id > 0 && matches!(m.media, Some(Media::Photo { .. })))
            .take(PHOTO_PREFETCH);
        for m in photos {
            let key = (m.chat_id, m.id);
            if self.photos.peek(&key).is_some() || !self.photo_loading.insert(key) {
                continue;
            }
            let api = api.clone();
            tasks.push(Task::perform(
                async move { api.download_photo(key.0, key.1).await },
                move |result| Message::Done(Done::Photo { key, result }),
            ));
        }
        Task::batch(tasks)
    }

    /// Put the chat's saved draft (text and reply target) into the composer.
    fn restore_draft(&mut self, chat: ChatId) {
        let draft = self.chats.get(chat).and_then(|c| c.draft.clone());
        match draft {
            Some(d) => {
                self.composer = d.text;
                self.compose_mode = d.reply_to.map_or(ComposeMode::Normal, ComposeMode::Reply);
            }
            None => {
                self.composer.clear();
                self.compose_mode = ComposeMode::Normal;
            }
        }
    }

    /// Store the composer as `chat`'s draft locally and on the server, if it
    /// differs from what is saved. While editing a message the composer is
    /// not a draft (the real draft was saved when the edit began).
    fn sync_draft(&mut self, chat: ChatId) -> Task<Message> {
        if matches!(self.compose_mode, ComposeMode::Edit(_)) || self.api.is_none() {
            return Task::none();
        }
        let draft = state::composer_draft(&self.composer, self.compose_mode, self.now_ts());
        let saved = self.chats.get(chat).and_then(|c| c.draft.clone());
        match (&draft, &saved) {
            (None, None) => return Task::none(),
            (Some(a), Some(b)) if a.same_content(b) => return Task::none(),
            _ => {}
        }
        self.chats.set_draft(chat, draft.clone());
        let local = draft.clone();
        let store = self.store_bg(move |s| s.set_draft(chat, local.as_ref()));
        let (text, reply) = draft.map_or((String::new(), None), |d| (d.text, d.reply_to));
        let net = self.api_call_quiet("draft", move |api| {
            Box::pin(async move { api.save_draft(chat, text, reply).await })
        });
        Task::batch([store, net])
    }

    /// The profile photo to draw for `chat`, if it is loaded and current.
    pub fn avatar(&self, chat: ChatId) -> Option<&image::Handle> {
        let pid = self.chats.get(chat)?.photo_id?;
        self.avatars
            .peek(&chat)
            .filter(|(p, _)| *p == pid)
            .map(|(_, h)| h)
    }

    /// Start loading missing profile photos for the top of the chat list
    /// (and the open chat): disk cache first, then the network, at most
    /// [`AVATAR_CONCURRENCY`] at a time. Works offline from the disk cache.
    pub fn fetch_avatars(&mut self) -> Task<Message> {
        if self.window_id.is_none() {
            return Task::none();
        }
        let api = self.api.clone();
        let wanted: Vec<(ChatId, i64)> = self
            .chats
            .all()
            .iter()
            .take(AVATAR_PREFETCH)
            .chain(self.selected.and_then(|id| self.chats.get(id)))
            .filter_map(|c| c.photo_id.map(|p| (c.id, p)))
            .collect();
        let mut tasks = Vec::new();
        for key in wanted {
            if self.avatar_loading.len() >= AVATAR_CONCURRENCY {
                break;
            }
            let have = self.avatars.peek(&key.0).is_some_and(|(p, _)| *p == key.1);
            if have || self.avatar_failed.contains(&key) || !self.avatar_loading.insert(key) {
                continue;
            }
            let (api, disk) = (api.clone(), self.avatar_disk.clone());
            tasks.push(Task::perform(
                async move {
                    let bytes = load_avatar(api, disk, key).await?;
                    // Decode (and cut the circle) off the UI thread.
                    tokio::task::spawn_blocking(move || crate::media::AvatarImage::decode(&bytes))
                        .await
                        .map_err(|_| Error::Cancelled)?
                        .ok_or_else(|| Error::Protocol("avatar undecodable".into()))
                },
                move |result| {
                    Message::Done(Done::Avatar {
                        chat: key.0,
                        photo_id: key.1,
                        result,
                    })
                },
            ));
        }
        Task::batch(tasks)
    }

    /// Delete cached avatars and opened files (logout / revoked session).
    fn wipe_media_caches(&self) -> Task<Message> {
        let disk = self.avatar_disk.clone();
        let open_dir = (!self.config.mock).then(|| self.config.open_dir());
        Task::perform(
            async move {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some(d) = disk {
                        d.clear();
                    }
                    if let Some(d) = open_dir {
                        let _ = std::fs::remove_dir_all(d);
                    }
                })
                .await;
            },
            |()| Message::Done(Done::Call(Ok(()))),
        )
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// Text for "typing…" in `chat`, if anybody is doing something there.
    pub fn typing_status(&self, chat: ChatId) -> Option<String> {
        let actions = self.typing.in_chat(chat);
        if actions.is_empty() {
            return None;
        }
        let private = chat > 0;
        let conv = self.conversation.as_ref().filter(|c| c.chat_id == chat);
        state::typing_text(&actions, private, |u| {
            self.user_names
                .get(&u)
                .cloned()
                .or_else(|| self.chats.user_name(u, conv))
        })
    }

    /// Remember who sent what, so typing statuses can show names.
    fn remember_names<'a>(&mut self, msgs: impl IntoIterator<Item = &'a ChatMessage>) {
        const MAX_NAMES: usize = 2_000;
        for m in msgs {
            if let (Some(id), Some(name)) = (m.sender_id, &m.sender_name) {
                if self.user_names.len() >= MAX_NAMES && !self.user_names.contains_key(&id) {
                    self.user_names.clear();
                }
                self.user_names.insert(id, name.clone());
            }
        }
    }

    /// Our own typing action after the composer changed.
    fn update_typing(&mut self) -> Task<Message> {
        let Some(chat) = self.selected else {
            return Task::none();
        };
        let has_text = !self.composer.trim().is_empty();
        let allowed = self.api.is_some()
            && !matches!(self.compose_mode, ComposeMode::Edit(_))
            && state::can_send_typing(self.chats.get(chat));
        match self
            .typing_throttle
            .on_input(chat, Self::now_ms(), has_text, allowed)
        {
            TypingStep::Nothing => Task::none(),
            TypingStep::Send => self.api_call_quiet("typing", move |api| {
                Box::pin(async move { api.set_typing(chat, true).await })
            }),
            TypingStep::Cancel(c) => self.api_call_quiet("typing", move |api| {
                Box::pin(async move { api.set_typing(c, false).await })
            }),
        }
    }

    fn toast(&mut self, text: String) -> Task<Message> {
        self.toast = Some(text);
        Task::perform(
            async { tokio::time::sleep(Duration::from_secs(4)).await },
            |_| Message::Ui(Ui::DismissToast),
        )
    }

    fn update_ui(&mut self, ui: Ui) -> Task<Message> {
        match ui {
            Ui::CallStart(_)
            | Ui::CallAccept
            | Ui::CallHangUp
            | Ui::CallToggleMute
            | Ui::CallExpand(_)
            | Ui::CallDevices(_)
            | Ui::CallPickDevice(..)
            | Ui::CallTick
            | Ui::CallEngine(_) => self.on_call_ui(ui),
            Ui::SetupApiId(s) => {
                if let Some(f) = &mut self.setup {
                    f.api_id = s.chars().filter(char::is_ascii_digit).take(12).collect();
                }
                Task::none()
            }
            Ui::SetupApiHash(s) => {
                if let Some(f) = &mut self.setup {
                    f.api_hash = s;
                }
                Task::none()
            }
            Ui::SetupSubmit => {
                let Some(f) = &mut self.setup else {
                    return Task::none();
                };
                let hash = f.api_hash.0.trim().to_ascii_lowercase();
                let (Some(api_id), true) = (
                    f.api_id.parse::<i32>().ok().filter(|n| *n > 0),
                    crate::config::valid_api_hash(&hash),
                ) else {
                    f.error =
                        Some("Enter the numeric api_id and the 32-character api_hash.".into());
                    return Task::none();
                };
                f.busy = true;
                f.error = None;
                let config = self.config.clone();
                let secrets = self.secrets.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            config.save_api_credentials(secrets.as_ref(), api_id, &hash)
                        })
                        .await
                        .map_err(|_| Error::Cancelled)?
                    },
                    |r| Message::Done(Done::CredentialsSaved(r)),
                )
            }
            Ui::OpenApiSetup => {
                if !self.auth.is_signed_in() {
                    self.setup = Some(SetupForm::new(true));
                    return operation::focus(Id::new(AUTH_INPUT_ID));
                }
                Task::none()
            }
            Ui::CancelSetup => {
                if self.setup.as_ref().is_some_and(|f| f.advanced) {
                    self.setup = None;
                    if self.api.is_none() && !self.config.mock {
                        return self.start_real_api();
                    }
                }
                Task::none()
            }
            Ui::UseBuiltinCredentials => {
                let Some(f) = &mut self.setup else {
                    return Task::none();
                };
                f.busy = true;
                let config = self.config.clone();
                let secrets = self.secrets.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            config.clear_api_override(secrets.as_ref())
                        })
                        .await
                        .map_err(|_| Error::Cancelled)?
                    },
                    |r| Message::Done(Done::CredentialsSaved(r)),
                )
            }
            Ui::AuthInput(s) => self.auth(AuthEvent::Input(s)),
            Ui::AuthSubmit => self.auth(AuthEvent::Submit),
            Ui::AuthBack => self.auth(AuthEvent::Back),

            Ui::SelectChat(id) => self.open_chat(id),
            Ui::SidebarSearch(q) => {
                self.sidebar_query = q.clone();
                self.sidebar_global.clear();
                self.global_searching = false;
                if q.trim().chars().count() < 2 {
                    self.sidebar_messages.clear();
                    return Task::none();
                }
                // The server-side search waits until typing pauses.
                let settled = q.trim().to_owned();
                let debounce = Task::perform(
                    async move {
                        tokio::time::sleep(Duration::from_millis(450)).await;
                        settled
                    },
                    |q| Message::Ui(Ui::GlobalSearchDue(q)),
                );
                let ids: Vec<ChatId> = self.chats.all().iter().map(|c| c.id).collect();
                let store = self.store.clone();
                let tag = q.clone();
                let local = Task::perform(
                    async move {
                        store
                            .call(move |s| {
                                let mut out = Vec::new();
                                for id in ids {
                                    out.extend(s.search_local(id, q.trim(), 5)?);
                                }
                                out.sort_by_key(|m| std::cmp::Reverse(m.date));
                                out.truncate(30);
                                Ok(out)
                            })
                            .await
                    },
                    move |result| {
                        Message::Done(Done::SidebarResults {
                            query: tag.clone(),
                            result,
                        })
                    },
                );
                Task::batch([local, debounce])
            }
            Ui::GlobalSearchDue(q) => {
                if q != self.sidebar_query.trim() || !self.auth.is_signed_in() {
                    return Task::none();
                }
                let Some(api) = self.api.clone() else {
                    return Task::none();
                };
                self.global_searching = true;
                info!(query = %Redacted(&q), "global search");
                let tag = q.clone();
                Task::perform(
                    async move { api.search_global(q, 30).await },
                    move |result| {
                        Message::Done(Done::GlobalResults {
                            query: tag.clone(),
                            result,
                        })
                    },
                )
            }
            Ui::SidebarScrolled(vp) => {
                let bottom = vp.absolute_offset().y + vp.bounds().height;
                if self.sidebar_query.trim().is_empty()
                    && vp.content_bounds().height - bottom < DIALOGS_LOAD_THRESHOLD
                {
                    self.load_more_dialogs()
                } else {
                    Task::none()
                }
            }
            Ui::ClearSidebarSearch => {
                self.sidebar_query.clear();
                self.sidebar_messages.clear();
                self.sidebar_global.clear();
                self.global_searching = false;
                Task::none()
            }
            Ui::OpenResult(chat, id) => {
                // A hit from a chat outside the loaded list: add it so the
                // header, avatar and replies have something to show.
                if self.chats.get(chat).is_none()
                    && let Some(h) = self
                        .sidebar_global
                        .iter()
                        .find(|h| h.message.chat_id == chat)
                {
                    self.chats.merge(vec![h.to_chat()]);
                }
                self.open_chat_at(chat, id)
            }

            Ui::HistoryScrolled(vp) => {
                let off = vp.absolute_offset().y;
                self.at_bottom = off < 60.0;
                let dist =
                    state::distance_from_top(off, vp.content_bounds().height, vp.bounds().height);
                if dist < state::LOAD_THRESHOLD {
                    self.load_older()
                } else {
                    Task::none()
                }
            }
            Ui::ComposerChanged(s) => {
                self.composer = s;
                let typing = self.update_typing();
                let Some(chat) = self.selected else {
                    return typing;
                };
                self.draft_version += 1;
                let version = self.draft_version;
                let due = Task::perform(
                    async move {
                        tokio::time::sleep(Duration::from_millis(state::DRAFT_SYNC_DELAY_MS)).await;
                    },
                    move |()| Message::Ui(Ui::DraftDue(chat, version)),
                );
                Task::batch([typing, due])
            }
            Ui::DraftDue(chat, version) => {
                if self.selected == Some(chat) && version == self.draft_version {
                    self.sync_draft(chat)
                } else {
                    Task::none()
                }
            }
            Ui::Send => {
                if self.pending_file.is_some() {
                    return self.update_ui(Ui::SendPendingFile);
                }
                self.send()
            }
            Ui::Attach => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("Send file")
                        .pick_file()
                        .await
                        .map(|f| f.path().to_path_buf())
                },
                |p| Message::Ui(Ui::FilePicked(p)),
            ),
            Ui::FilePicked(Some(path)) | Ui::FileDropped(path) => {
                if self.selected.is_none() || !self.auth.is_signed_in() {
                    return self.toast("Open a chat first to send a file.".into());
                }
                Task::perform(
                    async move {
                        let r = tokio::fs::metadata(&path)
                            .await
                            .map(|m| (m.len(), m.is_dir()))
                            .map_err(Error::from);
                        (path, r)
                    },
                    |(path, result)| Message::Done(Done::FileStat { path, result }),
                )
            }
            Ui::FilePicked(None) => Task::none(),
            Ui::SendPendingFile => match self.pending_file.take() {
                Some(f) => self.send_file(f),
                None => Task::none(),
            },
            Ui::CancelPendingFile => {
                self.drop_pending();
                Task::none()
            }
            Ui::ToggleInfo => {
                self.header_menu = None;
                if self.close_info() {
                    Task::none()
                } else {
                    self.open_info()
                }
            }
            Ui::InfoTab(kind) => {
                let Some(panel) = &mut self.info else {
                    return Task::none();
                };
                panel.select(kind);
                Task::batch([self.load_shared(kind), self.request_thumbs()])
            }
            Ui::Rescaled(f) => {
                let f = if f.is_finite() {
                    f.clamp(0.5, 4.0)
                } else {
                    1.0
                };
                let changed = info::thumb_side(f) != info::thumb_side(self.scale);
                self.scale = f;
                if changed && self.info.is_some() {
                    // Tiles of the old size are redone at the new one.
                    self.reset_thumbs();
                    return self.request_thumbs();
                }
                if changed {
                    self.reset_thumbs();
                }
                Task::none()
            }
            Ui::InfoScrolled(vp) => {
                // Remember where the panel is: the grid only builds (and
                // fetches thumbnails for) the rows around the viewport.
                if let Some(panel) = &mut self.info {
                    let grid_top = panel.grid_top_from_content(vp.content_bounds().height);
                    panel.set_view(vp.absolute_offset().y, vp.bounds().height, grid_top);
                }
                let bottom = vp.absolute_offset().y + vp.bounds().height;
                let more = if vp.content_bounds().height - bottom < INFO_LOAD_THRESHOLD
                    && let Some(kind) = self.info.as_ref().map(|p| p.tab)
                {
                    self.load_shared(kind)
                } else {
                    Task::none()
                };
                Task::batch([more, self.request_thumbs()])
            }
            Ui::SetNotifications(on) => self.chat_ui(Ui::SetMute(if on {
                crate::model::MuteFor::Off
            } else {
                crate::model::MuteFor::Forever
            })),
            Ui::OpenLink(url) => Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || crate::desktop::open_url(&url))
                        .await
                        .map_err(|_| Error::Cancelled)?
                },
                |r| Message::Done(Done::LinkOpened(r)),
            ),
            Ui::SelectFolder(id) => {
                self.folders.selected = id.filter(|i| self.folders.get(*i).is_some());
                self.folders.tab_menu = None;
                self.folders.chat_menu = None;
                self.fill_folder()
            }
            Ui::FolderTabMenu(id) => {
                self.folders.chat_menu = None;
                self.folders.tab_menu = if self.folders.tab_menu == id {
                    None
                } else {
                    id
                };
                Task::none()
            }
            Ui::ChatMenu(id) => {
                self.folders.tab_menu = None;
                self.folders.chat_menu = if self.folders.chat_menu == id {
                    None
                } else {
                    id
                };
                Task::none()
            }
            Ui::ChatInFolder(chat, fid) => {
                self.folders.chat_menu = None;
                self.header_menu = None;
                let (Some(c), Some(mut f)) = (
                    self.chats.get(chat).cloned(),
                    self.folders.get(fid).cloned(),
                ) else {
                    return Task::none();
                };
                if f.shared {
                    return self.toast("Shared folders are edited in the official app.".into());
                }
                f.toggle_chat(&c);
                if f.explicit_len() > crate::model::FOLDER_CHATS_MAX {
                    return self
                        .toast(Error::Protocol("FILTER_INCLUDE_TOO_MUCH".into()).user_message());
                }
                self.save_folder_optimistic(f)
            }
            Ui::OpenFolders => {
                self.main_menu = false;
                self.folders.manager_open = true;
                self.folders.confirm_delete = None;
                Task::none()
            }
            Ui::CloseFolders => {
                self.folders.manager_open = false;
                self.folders.confirm_delete = None;
                Task::none()
            }
            Ui::FolderMove(id, delta) => {
                let before = self.folders.list.clone();
                let Some(order) = self.folders.step(id, delta) else {
                    return Task::none();
                };
                let persist = self.persist_folders();
                let Some(api) = self.api.clone() else {
                    return persist;
                };
                Task::batch([
                    persist,
                    Task::perform(
                        async move { api.reorder_dialog_filters(order).await },
                        move |result| Message::Done(Done::FolderOp { before, result }),
                    ),
                ])
            }
            Ui::FolderDeleteAsk(id) => {
                // Confirmed inside the folder manager (also from a tab menu).
                self.folders.tab_menu = None;
                self.folders.manager_open = true;
                self.folders.confirm_delete = Some(id);
                Task::none()
            }
            Ui::FolderDeleteCancel => {
                self.folders.confirm_delete = None;
                Task::none()
            }
            Ui::FolderDeleteConfirm(id) => {
                self.folders.confirm_delete = None;
                self.folders.tab_menu = None;
                let before = self.folders.list.clone();
                if self.folders.remove(id).is_none() {
                    return Task::none();
                }
                let persist = self.persist_folders();
                let Some(api) = self.api.clone() else {
                    return persist;
                };
                Task::batch([
                    persist,
                    Task::perform(
                        async move { api.delete_dialog_filter(id).await },
                        move |result| Message::Done(Done::FolderOp { before, result }),
                    ),
                ])
            }
            Ui::FolderNew(chat) => {
                self.header_menu = None;
                self.folders.chat_menu = None;
                self.folders.tab_menu = None;
                let mut ed = folders::FolderEditor::create(self.folders.next_id());
                if let Some(c) = chat {
                    ed.toggle_chat(folders::Picker::Include, c);
                }
                self.folder_editor = Some(ed);
                operation::focus(Id::new(FOLDER_TITLE_ID))
            }
            Ui::FolderEdit(id) => {
                self.folders.tab_menu = None;
                let Some(f) = self.folders.get(id).cloned() else {
                    return Task::none();
                };
                if f.shared {
                    return self.toast("Shared folders are edited in the official app.".into());
                }
                self.folder_editor = Some(folders::FolderEditor::edit(f));
                operation::focus(Id::new(FOLDER_TITLE_ID))
            }
            Ui::FolderTitle(t) => {
                if let Some(ed) = &mut self.folder_editor {
                    ed.set_title(&t);
                }
                Task::none()
            }
            Ui::FolderFlag(flag) => {
                if let Some(ed) = &mut self.folder_editor {
                    ed.toggle(flag);
                }
                Task::none()
            }
            Ui::FolderPick(which) => {
                if let Some(ed) = &mut self.folder_editor {
                    ed.picker = which;
                    ed.query.clear();
                }
                Task::none()
            }
            Ui::FolderPickQuery(q) => {
                if let Some(ed) = &mut self.folder_editor {
                    ed.query = q;
                }
                Task::none()
            }
            Ui::FolderChatToggle(which, chat) => {
                if let Some(ed) = &mut self.folder_editor {
                    ed.toggle_chat(which, chat);
                }
                Task::none()
            }
            Ui::FolderChatRemove(which, chat) => {
                if let Some(ed) = &mut self.folder_editor {
                    ed.remove_chat(which, chat);
                }
                Task::none()
            }
            Ui::FolderEditorClose => {
                self.folder_editor = None;
                Task::none()
            }
            Ui::FolderSave => {
                let Some(ed) = &mut self.folder_editor else {
                    return Task::none();
                };
                if ed.saving {
                    return Task::none();
                }
                if let Err(e) = ed.validate() {
                    ed.error = Some(e.message());
                    return Task::none();
                }
                let folder = ed.to_save();
                let Some(api) = self.api.clone() else {
                    ed.error = Some(Error::Net("not connected".into()).user_message());
                    return Task::none();
                };
                ed.saving = true;
                ed.error = None;
                let sent = folder.clone();
                Task::perform(
                    async move { api.save_dialog_filter(sent).await },
                    move |result| {
                        Message::Done(Done::FolderSaved {
                            folder: folder.clone(),
                            result,
                        })
                    },
                )
            }
            Ui::Paste => {
                if self.selected.is_none()
                    || !self.auth.is_signed_in()
                    || self.settings_open
                    || self.viewer.is_some()
                {
                    return Task::none();
                }
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(crate::clipboard::read_blocking)
                            .await
                            .unwrap_or(crate::clipboard::Clip::Nothing)
                    },
                    |c| Message::Done(Done::Pasted(c)),
                )
            }
            Ui::TextPasted(s) => {
                self.last_paste = Some((s, Self::now_ms()));
                Task::none()
            }
            Ui::CopyImage(chat, id) => self.copy_image(chat, id),
            Ui::ToggleEmoji => {
                self.emoji_open = !self.emoji_open;
                Task::none()
            }
            Ui::InsertEmoji(e) => {
                self.composer.push_str(e);
                Task::batch([
                    operation::focus(Id::new(COMPOSER_ID)),
                    operation::move_cursor_to_end(Id::new(COMPOSER_ID)),
                ])
            }
            Ui::MessageMenu(id) => {
                // No actions on messages that are still being sent.
                let id = id.filter(|i| *i > 0);
                self.menu_for = if self.menu_for == id { None } else { id };
                Task::none()
            }
            Ui::Reply(id) if id > 0 => {
                self.menu_for = None;
                self.compose_mode = ComposeMode::Reply(id);
                operation::focus(Id::new(COMPOSER_ID))
            }
            Ui::Edit(id) if id > 0 => {
                self.menu_for = None;
                // Keep what was being typed as the draft while editing.
                let stash = match self.selected {
                    Some(chat) => self.sync_draft(chat),
                    None => Task::none(),
                };
                if let Some(m) = self.conversation.as_ref().and_then(|c| c.get(id)) {
                    self.composer = m.text.clone();
                    self.compose_mode = ComposeMode::Edit(id);
                }
                Task::batch([
                    stash,
                    operation::focus(Id::new(COMPOSER_ID)),
                    operation::move_cursor_to_end(Id::new(COMPOSER_ID)),
                ])
            }
            Ui::Delete(id) if id > 0 => {
                self.menu_for = None;
                let Some(conv) = &mut self.conversation else {
                    return Task::none();
                };
                let chat = conv.chat_id;
                let Some(old) = conv.get(id).cloned() else {
                    return Task::none();
                };
                conv.remove_local(id);
                self.api_call_or_rollback(old, move |api| {
                    Box::pin(async move { api.delete_messages(chat, vec![id]).await })
                })
            }
            Ui::React(id, emoji) if id > 0 => {
                self.menu_for = None;
                let Some(conv) = &mut self.conversation else {
                    return Task::none();
                };
                let chat = conv.chat_id;
                let Some(old) = conv.get(id).cloned() else {
                    return Task::none();
                };
                match conv.toggle_reaction(id, &emoji) {
                    Some(target) => self.api_call_or_rollback(old, move |api| {
                        Box::pin(async move { api.react(chat, id, target).await })
                    }),
                    None => Task::none(),
                }
            }
            Ui::Reply(_) | Ui::Edit(_) | Ui::Delete(_) | Ui::React(..) => Task::none(),
            Ui::Copy(id) => {
                self.menu_for = None;
                match self.conversation.as_ref().and_then(|c| c.get(id)) {
                    Some(m) => iced::clipboard::write(m.text.clone()),
                    None => Task::none(),
                }
            }
            Ui::SaveMedia(chat, id) => {
                self.menu_for = None;
                let suggested = self
                    .conversation
                    .as_ref()
                    .and_then(|c| c.get(id))
                    .and_then(|m| match &m.media {
                        Some(Media::File { name, .. }) => Some(name.clone()),
                        Some(Media::Photo { .. }) => Some(format!("photo-{id}.jpg")),
                        _ => None,
                    })
                    .unwrap_or_else(|| format!("file-{id}"));
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title("Save file")
                            .set_file_name(suggested)
                            .save_file()
                            .await
                            .map(|f| f.path().to_path_buf())
                    },
                    move |path| Message::Done(Done::SavePicked { chat, id, path }),
                )
            }
            Ui::CancelCompose => {
                if matches!(self.compose_mode, ComposeMode::Edit(_)) {
                    // Back to the draft that was stashed when the edit began.
                    self.compose_mode = ComposeMode::Normal;
                    if let Some(chat) = self.selected {
                        self.restore_draft(chat);
                    } else {
                        self.composer.clear();
                    }
                    return Task::none();
                }
                self.compose_mode = ComposeMode::Normal;
                match self.selected {
                    Some(chat) => self.sync_draft(chat),
                    None => Task::none(),
                }
            }
            Ui::ToggleChatSearch
            | Ui::OpenChatSearch
            | Ui::ChatSearchInput(_)
            | Ui::ChatSearchSubmit
            | Ui::ChatSearchDue(_)
            | Ui::ChatSearchStep(_)
            | Ui::ChatSearchFrom(_)
            | Ui::ChatSearchFromPicker
            | Ui::ChatSearchFromQuery(_)
            | Ui::ToggleHeaderMenu
            | Ui::HeaderMenuPage(_)
            | Ui::CloseHeaderMenu
            | Ui::SetMute(_)
            | Ui::AskClearHistory
            | Ui::AskDeleteChat
            | Ui::AskLeave
            | Ui::ConfirmRevoke(_)
            | Ui::ConfirmAccept
            | Ui::ConfirmCancel
            | Ui::ToggleBlock
            | Ui::StartSelect
            | Ui::ToggleSelect(_)
            | Ui::CancelSelect
            | Ui::AskDeleteSelected
            | Ui::CopySelected => self.chat_ui(ui),
            Ui::ScrollToBottom => {
                if self.conversation.as_ref().is_some_and(|c| c.detached) {
                    return self.reattach();
                }
                self.at_bottom = true;
                operation::snap_to(Id::new(HISTORY_ID), operation::RelativeOffset::START)
            }
            Ui::OpenMedia(chat, id) => {
                self.viewer = Some((chat, id));
                // From the info panel the photo may not be loaded yet.
                self.ensure_photo((chat, id))
            }
            Ui::CloseMedia => {
                self.viewer = None;
                Task::none()
            }
            Ui::ToggleMainMenu => {
                self.main_menu = !self.main_menu;
                Task::none()
            }
            ui @ (Ui::UpdateCheckDue(_)
            | Ui::UpdateCheckNow
            | Ui::UpdateInstall
            | Ui::UpdateCancel
            | Ui::UpdateDismiss
            | Ui::UpdateRestart
            | Ui::UpdateReleaseNotes
            | Ui::UpdateCopyCommand
            | Ui::SetCheckUpdates(_)) => self.update_updates(ui),
            Ui::DismissToast => {
                self.toast = None;
                Task::none()
            }
            Ui::Remote(request) => self.on_remote(request),
            Ui::CloseRequested => {
                let action = self.window.close_requested();
                self.apply_window(action)
            }
            Ui::WindowResized(size) => {
                self.geometry.resized(size.width, size.height);
                Task::none()
            }
            Ui::WindowMoved(p) => {
                self.geometry.moved(p.x, p.y);
                Task::none()
            }
            Ui::OpenSettings => {
                self.main_menu = false;
                self.settings_open = true;
                Task::none()
            }
            Ui::CloseSettings => {
                self.settings_open = false;
                Task::none()
            }
            Ui::SetCloseToTray(on) => {
                self.prefs.close_to_tray = on;
                self.window.close_to_tray = on;
                self.save_prefs()
            }
            Ui::SetStartMinimized(on) => {
                self.prefs.start_minimized = on;
                self.save_prefs()
            }
            Ui::SetAutostart(on) => {
                if self.config.mock {
                    // Demo mode never writes outside its own process.
                    self.autostart = on;
                    return Task::none();
                }
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || crate::autostart::set_enabled(on))
                            .await
                            .map_err(|_| Error::Cancelled)?
                            .map_err(|e| Error::Io(format!("autostart: {}", e.kind())))
                    },
                    |r| Message::Done(Done::AutostartSet(r)),
                )
            }
            Ui::LogOut => {
                self.main_menu = false;
                let api = self.api.clone();
                let store = self.store.clone();
                let media = self.wipe_media_caches();
                Task::batch([
                    media,
                    Task::perform(
                        async move {
                            if let Some(api) = api {
                                let _ = api.log_out().await;
                            }
                            let _ = store.call(|s| s.clear()).await;
                        },
                        |_| Message::Done(Done::LoggedOut),
                    ),
                ])
            }
            Ui::Focus(f) => {
                self.focused = f;
                if f && let Some(chat) = self.selected {
                    let had_unread = self.chats.get(chat).is_some_and(|c| c.unread_count > 0);
                    self.chats.mark_read(chat);
                    if had_unread {
                        return self.api_call(move |api| {
                            Box::pin(async move { api.mark_read(chat).await })
                        });
                    }
                }
                Task::none()
            }
            Ui::Escape => {
                // Close the top-most layer only, like the official client.
                if self.setup.as_ref().is_some_and(|f| f.advanced) {
                    return self.update_ui(Ui::CancelSetup);
                }
                if let Some(ed) = &mut self.folder_editor {
                    // Innermost first: the chat picker, then the editor.
                    if ed.picker.take().is_none() {
                        self.folder_editor = None;
                    }
                    return Task::none();
                }
                let closed = self.confirm.take().is_some()
                    || self.header_menu.take().is_some()
                    || std::mem::take(&mut self.folders.manager_open)
                    || self.folders.chat_menu.take().is_some()
                    || self.folders.tab_menu.take().is_some()
                    || std::mem::take(&mut self.settings_open)
                    || std::mem::take(&mut self.main_menu)
                    || self.viewer.take().is_some()
                    || self.menu_for.take().is_some()
                    || self.drop_pending()
                    || std::mem::take(&mut self.emoji_open)
                    || self.close_info();
                if closed {
                    Task::none()
                } else if self.compose_mode != ComposeMode::Normal {
                    self.update_ui(Ui::CancelCompose)
                } else if self.selection.take().is_some() {
                    Task::none()
                } else if self.chat_search.take().is_some() {
                    self.highlight = None;
                    Task::none()
                } else if !self.sidebar_query.is_empty() {
                    self.update_ui(Ui::ClearSidebarSearch)
                } else {
                    Task::none()
                }
            }
            Ui::TypingTick => {
                self.typing.expire(Self::now_ms());
                Task::none()
            }
            Ui::OpenFile(chat, id) => self.open_file(chat, id),
            Ui::Tick => {
                if !self.first_frame_logged {
                    self.first_frame_logged = true;
                    let ms = self.started.elapsed().as_secs_f64() * 1000.0;
                    info!(first_frame_ms = format!("{ms:.1}"), "first frame");
                    eprintln!("first_frame_ms={ms:.1}");
                }
                if !self.config.mock {
                    self.now = chrono::Utc::now().timestamp();
                }
                // Timed mutes run out: badges, tray count and notifications
                // follow.
                self.chats.refresh_notify(chrono::Utc::now().timestamp());
                Task::none()
            }
        }
    }

    fn send(&mut self) -> Task<Message> {
        let text = self.composer.trim().to_owned();
        let Some(chat) = self.selected else {
            return Task::none();
        };
        if text.is_empty() {
            return Task::none();
        }
        self.composer.clear();
        self.typing_throttle.reset();
        self.emoji_open = false;
        // Sending from an older window returns to the newest messages first.
        let reattach = if self.conversation.as_ref().is_some_and(|c| c.detached) {
            self.reattach()
        } else {
            Task::none()
        };
        let mode = std::mem::replace(&mut self.compose_mode, ComposeMode::Normal);
        if let ComposeMode::Edit(id) = mode {
            let Some(conv) = &mut self.conversation else {
                return Task::none();
            };
            let Some(old) = conv.get(id).cloned() else {
                return Task::none();
            };
            conv.edit_local(id, &text);
            self.restore_draft(chat);
            return self.api_call_or_rollback(old, move |api| {
                Box::pin(async move { api.edit_message(chat, id, text).await })
            });
        }
        // A sent message consumes the draft (the server clears it too).
        self.chats.set_draft(chat, None);
        let clear_draft = self.store_bg(move |s| s.set_draft(chat, None));
        let reply_to = match mode {
            ComposeMode::Reply(id) => Some(id),
            _ => None,
        };
        let mut pending = ChatMessage::text(chat, 0, true, self.now_ts(), &text);
        pending.reply_to = reply_to;
        pending.sender_id = self.me.as_ref().map(|u| u.id);
        let temp = match &mut self.conversation {
            Some(conv) => conv.push_pending(pending.clone()),
            None => return Task::none(),
        };
        pending.id = temp;
        pending.state = SendState::Sending;
        self.chats.set_last(&pending);
        self.at_bottom = true;
        let Some(api) = self.api.clone() else {
            return Task::done(Message::Done(Done::Sent {
                chat,
                temp,
                result: Err(Error::Net("not connected".into())),
            }));
        };
        Task::batch([
            reattach,
            clear_draft,
            Task::perform(
                async move { api.send_text(chat, text, reply_to).await },
                move |result| Message::Done(Done::Sent { chat, temp, result }),
            ),
            operation::snap_to(Id::new(HISTORY_ID), operation::RelativeOffset::START),
        ])
    }

    /// Forget the pending file; a pasted temp image is deleted. `true` when
    /// there was one.
    fn drop_pending(&mut self) -> bool {
        let Some(f) = self.pending_file.take() else {
            return false;
        };
        if f.temp {
            // A tiny unlink on a plain thread: no runtime needed in `update`.
            std::thread::spawn(move || {
                let _ = std::fs::remove_file(f.path);
            });
        }
        true
    }

    /// The composer pasted text for the same Ctrl+V (a copied file's path):
    /// take it out again, the file is what the user meant.
    fn undo_text_paste(&mut self) {
        if let Some((pasted, at)) = self.last_paste.take()
            && Self::now_ms() - at < 3_000
        {
            self.composer = crate::clipboard::strip_pasted(&self.composer, &pasted);
        }
    }

    fn on_pasted(&mut self, clip: crate::clipboard::Clip) -> Task<Message> {
        use crate::clipboard::Clip;
        if self.selected.is_none() || !self.auth.is_signed_in() {
            return Task::none();
        }
        match clip {
            Clip::Nothing => Task::none(),
            Clip::Files(files) => {
                self.undo_text_paste();
                let n = files.len();
                let Some(first) = files.into_iter().next() else {
                    return Task::none();
                };
                let attach = self.update_ui(Ui::FilePicked(Some(first)));
                if n > 1 {
                    let t =
                        self.toast("Only one file is sent at a time: attached the first.".into());
                    Task::batch([attach, t])
                } else {
                    attach
                }
            }
            Clip::Image(png) => {
                self.undo_text_paste();
                let dir = self.config.paste_dir();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            crate::clipboard::save_pasted(&dir, &png)
                        })
                        .await
                        .map_err(|_| Error::Cancelled)?
                        .map_err(Error::from)
                    },
                    |r| Message::Done(Done::PasteSaved(r)),
                )
            }
        }
    }

    /// "Copy image": the cached photo bytes, or a fresh download, decoded
    /// (with the usual caps) and put on the system clipboard.
    fn copy_image(&mut self, chat: ChatId, id: MessageId) -> Task<Message> {
        self.menu_for = None;
        let cached = match self.photos.peek(&(chat, id)) {
            Some(image::Handle::Bytes(_, b)) => Some(b.to_vec()),
            _ => None,
        };
        let api = self.api.clone();
        Task::perform(
            async move {
                let bytes = match (cached, api) {
                    (Some(b), _) => b,
                    (None, Some(api)) => api.download_photo(chat, id).await?,
                    (None, None) => return Err(Error::Net("not connected".into())),
                };
                tokio::task::spawn_blocking(move || {
                    let px = crate::clipboard::decode_photo(&bytes)
                        .ok_or_else(|| Error::Protocol("image not copyable".into()))?;
                    crate::clipboard::write_image_blocking(px)
                        .map_err(|_| Error::Io("clipboard unavailable".into()))
                })
                .await
                .map_err(|_| Error::Cancelled)?
            },
            |r| Message::Done(Done::ImageCopied(r)),
        )
    }

    fn send_file(&mut self, file: PendingFile) -> Task<Message> {
        let Some(chat) = self.selected else {
            return Task::none();
        };
        let caption = std::mem::take(&mut self.composer);
        self.typing_throttle.reset();
        self.chats.set_draft(chat, None);
        let reply_to = match std::mem::replace(&mut self.compose_mode, ComposeMode::Normal) {
            ComposeMode::Reply(id) => Some(id),
            _ => None,
        };
        let mut pending = ChatMessage::text(chat, 0, true, self.now_ts(), &caption);
        pending.reply_to = reply_to;
        pending.media = Some(Media::File {
            name: file.name.clone(),
            size: file.size,
            mime: String::new(),
        });
        let temp = match &mut self.conversation {
            Some(conv) => conv.push_pending(pending),
            None => return Task::none(),
        };
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        let PendingFile {
            path,
            kind,
            temp: is_paste,
            ..
        } = file;
        let clear_draft = self.store_bg(move |s| s.set_draft(chat, None));
        Task::batch([
            clear_draft,
            Task::perform(
                async move {
                    let r = api
                        .send_file(chat, path.clone(), kind, caption, reply_to)
                        .await;
                    if is_paste {
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                    r
                },
                move |result| Message::Done(Done::Sent { chat, temp, result }),
            ),
        ])
    }

    /// Click on a file: download it into the private open-cache and hand it
    /// to the system handler. Executable types are save-only.
    fn open_file(&mut self, chat: ChatId, id: MessageId) -> Task<Message> {
        self.menu_for = None;
        let Some((name, size)) = self
            .conversation
            .as_ref()
            .and_then(|c| c.get(id))
            .and_then(|m| match &m.media {
                Some(Media::File { name, size, .. }) => Some((name.clone(), *size)),
                _ => None,
            })
        else {
            return Task::none();
        };
        if id <= 0 {
            return Task::none();
        }
        if crate::media::is_save_only(&name) {
            let t = self.toast(
                "For your safety this file type is not opened automatically. Choose where to save it."
                    .into(),
            );
            return Task::batch([t, self.update_ui(Ui::SaveMedia(chat, id))]);
        }
        if size > crate::media::MAX_OPEN_BYTES {
            return self.toast("This file is too large to open directly. Use “Save as…”.".into());
        }
        let Some(api) = self.api.clone() else {
            return Task::none();
        };
        let path = crate::media::open_path(&self.config.open_dir(), chat, id, &name);
        Task::perform(
            async move {
                if let Some(dir) = path.parent() {
                    let d = dir.to_path_buf();
                    tokio::task::spawn_blocking(move || crate::config::ensure_private_dir(&d))
                        .await
                        .map_err(|_| Error::Cancelled)??;
                }
                // Reuse an earlier download of the same message.
                if tokio::fs::metadata(&path).await.is_ok_and(|m| m.len() > 0) {
                    return Ok(path);
                }
                api.download_media(chat, id, path.clone()).await?;
                crate::media::make_non_executable(&path);
                Ok(path)
            },
            |r| Message::Done(Done::FileReady(r)),
        )
    }

    fn now_ts(&self) -> i64 {
        if self.config.mock && self.config.screenshot.is_some() {
            self.now
        } else {
            chrono::Utc::now().timestamp()
        }
    }

    // ------------------------------------------------------------- events

    fn on_event(&mut self, ev: Event) -> Task<Message> {
        let open = self.selected.filter(|_| self.focused);
        let mut tasks = Vec::new();
        match &ev {
            Event::Connection(c) => {
                self.conn = *c;
                return Task::none();
            }
            Event::PhoneCall(_) | Event::CallSignaling { .. } => return self.on_call_event(&ev),
            Event::SessionRevoked => return self.session_revoked(),
            Event::FolderChanged { id, folder } => {
                self.folders.apply_changed(*id, folder.clone());
                return Task::batch([self.persist_folders(), self.fill_folder()]);
            }
            Event::FoldersOrder(order) => {
                self.folders.apply_order(order);
                return self.persist_folders();
            }
            Event::FoldersStale => return self.fetch_folders(),
            Event::NewMessage(m) => {
                self.typing.message_from(m.chat_id, m.sender_id);
                self.remember_names(std::iter::once(m));
                if self.config.notifications
                    && !self.muted()
                    && state::should_notify(
                        self.chats.get(m.chat_id),
                        m,
                        self.selected,
                        self.focused,
                        chrono::Utc::now().timestamp(),
                    )
                {
                    let eff = self
                        .chats
                        .effective(m.chat_id, chrono::Utc::now().timestamp());
                    let (title, body) = state::notification_text(
                        self.chats.get(m.chat_id),
                        m,
                        self.config.notification_preview && eff.is_none_or(|e| e.show_previews),
                    );
                    crate::desktop::notify(crate::desktop::Toast {
                        title,
                        body,
                        chat: Some(m.chat_id),
                        silent: eff.is_some_and(|e| e.silent),
                    });
                }
                let m2 = m.clone();
                tasks.push(self.store_bg(move |s| s.upsert_messages(&[m2])));
                if open == Some(m.chat_id) && !m.outgoing {
                    let chat = m.chat_id;
                    tasks.push(
                        self.api_call(move |api| {
                            Box::pin(async move { api.mark_read(chat).await })
                        }),
                    );
                }
            }
            Event::MessageEdited(m) => {
                let m2 = m.clone();
                tasks.push(self.store_bg(move |s| s.upsert_messages(&[m2])));
            }
            Event::MessagesDeleted { chat, ids } => {
                let (chat, ids) = (*chat, ids.clone());
                tasks.push(self.store_bg(move |s| s.delete_messages(chat, &ids).map(|_| ())));
            }
            Event::Reactions {
                chat,
                id,
                reactions,
            } => {
                let (chat, id, r) = (*chat, *id, reactions.clone());
                tasks.push(self.store_bg(move |s| s.set_reactions(chat, id, &r)));
            }
            Event::ReadOutbox { chat, max_id } => {
                let (chat, max_id) = (*chat, *max_id);
                tasks.push(self.store_bg(move |s| s.set_read_outbox(chat, max_id)));
            }
            Event::Draft { chat, draft } => {
                let (chat, d) = (*chat, draft.clone());
                // Another device changed the draft: show it unless the user
                // is typing here right now.
                if self.selected == Some(chat)
                    && self.composer.trim().is_empty()
                    && !matches!(self.compose_mode, ComposeMode::Edit(_))
                {
                    self.composer = d.as_ref().map_or_else(String::new, |d| d.text.clone());
                    self.compose_mode = d
                        .as_ref()
                        .and_then(|d| d.reply_to)
                        .map_or(ComposeMode::Normal, ComposeMode::Reply);
                }
                tasks.push(self.store_bg(move |s| s.set_draft(chat, d.as_ref())));
            }
            Event::Typing { chat, user, action } => {
                if Some(*user) != self.me.as_ref().map(|u| u.id) {
                    self.typing.apply(*chat, *user, *action, Self::now_ms());
                }
            }
            Event::Notify { target, settings } => {
                let settings = *settings;
                match *target {
                    crate::model::NotifyTarget::Chat(chat) => {
                        tasks.push(self.store_bg(move |s| s.set_notify(chat, &settings)));
                    }
                    crate::model::NotifyTarget::Scope(scope) => {
                        let mut d = *self.chats.defaults();
                        d.set(scope, settings);
                        tasks.push(self.store_bg(move |s| s.save_notify_defaults(&d)));
                    }
                }
            }
            Event::ReadInbox { .. } | Event::UserStatus { .. } => {}
        }
        self.chats.apply_event(&ev, open);
        if let Some(conv) = &mut self.conversation {
            conv.apply_event(&ev);
            if self.at_bottom {
                conv.trim();
            }
        }
        if matches!(ev, Event::NewMessage(_)) {
            tasks.push(self.fetch_photos());
        }
        Task::batch(tasks)
    }

    /// The server revoked our session: wipe secrets and cache off the UI
    /// thread, and build a fresh client for the next login.
    fn session_revoked(&mut self) -> Task<Message> {
        warn!("session revoked");
        let vault = Vault::new(self.secrets.clone(), self.config.session_blob_path());
        let store = self.store.clone();
        let wipe = Task::perform(
            async move {
                let _ = tokio::task::spawn_blocking(move || vault.clear()).await;
                store.call(|s| s.clear()).await
            },
            |r| Message::Done(Done::Call(r)),
        );
        let media = self.wipe_media_caches();
        self.reset_session();
        crate::auth::reduce(&mut self.auth, AuthEvent::LoggedOut);
        Task::batch([wipe, media, self.rebuild_api()])
    }

    /// Replace the client after logout/revoke so nothing from the old
    /// session (auth key, update pump, event channel) is reused.
    fn rebuild_api(&mut self) -> Task<Message> {
        if self.config.mock {
            // The fake backend supports logout → login on the same instance;
            // its event stream restarts on the next subscription.
            self.api_gen += 1;
            return Task::none();
        }
        self.api = None;
        self.start_real_api()
    }

    fn reset_session(&mut self) {
        self.me = None;
        self.chats = ChatList::default();
        self.selected = None;
        self.conversation = None;
        self.composer.clear();
        self.compose_mode = ComposeMode::Normal;
        self.drop_pending();
        self.chat_search = None;
        self.header_menu = None;
        self.confirm = None;
        self.selection = None;
        self.blocked.clear();
        self.sidebar_messages.clear();
        self.sidebar_global.clear();
        self.global_searching = false;
        self.photos = Lru::new(IMAGE_BUDGET);
        self.photo_loading.clear();
        self.avatars = Lru::new(AVATAR_BUDGET);
        self.avatar_loading.clear();
        self.avatar_failed.clear();
        self.typing.clear();
        self.typing_throttle.reset();
        self.dialogs_exhausted = false;
        self.dialogs_loading = false;
        self.folders = folders::FolderState::default();
        self.folder_editor = None;
        self.info = None;
        self.reset_thumbs();
    }

    fn on_done(&mut self, done: Done) -> Task<Message> {
        match done {
            Done::CallConfig(_)
            | Done::CallPlaced(_)
            | Done::CallAccepted(_)
            | Done::CallConfirmed(_)
            | Done::CallKey(_)
            | Done::CallMedia(_)
            | Done::CallDevicesListed(..)
            | Done::CallSent(_) => self.on_call_done(done),
            Done::ApiReady(Ok(api)) => {
                self.api = Some(api.clone());
                self.api_gen += 1;
                if !self.setup.as_ref().is_some_and(|f| f.advanced) {
                    self.setup = None;
                }
                let notice = api.take_notice().map_or_else(Task::none, |n| self.toast(n));
                let restore = Task::perform(async move { api.restore().await }, |r| {
                    Message::Done(Done::Restored(r))
                });
                Task::batch([notice, restore])
            }
            Done::ApiReady(Err(Error::Auth(AuthError::MissingApiCredentials))) => {
                // Only development builds without embedded credentials get here.
                self.conn = ConnState::Disconnected;
                self.setup = Some(SetupForm::new(false));
                Task::none()
            }
            Done::ApiReady(Err(e)) => {
                warn!(error = %e, "telegram client unavailable");
                self.conn = ConnState::Disconnected;
                self.auth(AuthEvent::Restored(Err(e)))
            }
            Done::CredentialsSaved(Ok(())) => {
                self.setup = None;
                if self.config.mock {
                    return Task::none();
                }
                // Different app credentials: shut the old client down (this
                // also clears its unauthorised session) and start a new one.
                let old = self.api.take();
                Task::perform(
                    async move {
                        if let Some(api) = old {
                            let _ = api.log_out().await;
                        }
                    },
                    |_| Message::Done(Done::RestartApi),
                )
            }
            Done::RestartApi => {
                self.reset_session();
                self.auth = AuthState::Restoring;
                self.start_real_api()
            }
            Done::CredentialsSaved(Err(e)) => {
                if let Some(f) = &mut self.setup {
                    f.busy = false;
                    f.error = Some(e.user_message());
                }
                Task::none()
            }
            Done::Restored(Err(e)) if is_transient(&e) && self.auth == AuthState::Restoring => {
                // Offline with a possibly valid session: keep showing the
                // cache and retry with backoff; never fall back to the phone
                // screen (that would cost an SMS and risk FLOOD_WAIT).
                self.restore_attempt += 1;
                self.conn = ConnState::Reconnecting {
                    attempt: self.restore_attempt,
                };
                let delay = backoff::delay(self.restore_attempt, backoff::jitter());
                warn!(attempt = self.restore_attempt, "restore failed, retrying");
                Task::perform(async move { tokio::time::sleep(delay).await }, |_| {
                    Message::Done(Done::RetryRestore)
                })
            }
            Done::RetryRestore => {
                let Some(api) = self.api.clone() else {
                    return Task::none();
                };
                self.conn = ConnState::Connecting;
                Task::perform(async move { api.restore().await }, |r| {
                    Message::Done(Done::Restored(r))
                })
            }
            Done::Restored(r) => {
                let user = r.as_ref().ok().cloned().flatten();
                if let Err(e) = &r
                    && e.is_session_revoked()
                {
                    return self.session_revoked();
                }
                let mut t = Task::none();
                if matches!(r, Ok(None)) && !self.chats.all().is_empty() {
                    // Definitely logged out: drop the stale cache.
                    self.reset_session();
                    t = self.store_bg(|s| s.clear());
                }
                let t = Task::batch([t, self.auth(AuthEvent::Restored(r))]);
                if let Some(u) = user {
                    return Task::batch([t, self.on_signed_in(Some(u))]);
                }
                if !self.auth.is_signed_in() {
                    self.conn = ConnState::Connected;
                    if self.config.screenshot.is_none() {
                        return Task::batch([t, operation::focus(Id::new(AUTH_INPUT_ID))]);
                    }
                }
                t
            }
            Done::CodeRequested(r) => {
                let ok = r.is_ok();
                let t = self.auth(AuthEvent::CodeRequested(r));
                if ok {
                    Task::batch([t, operation::focus(Id::new(AUTH_INPUT_ID))])
                } else {
                    t
                }
            }
            Done::SignIn(r) => {
                let user = match &r {
                    Ok(SignInOutcome::SignedIn(u)) => Some(u.clone()),
                    _ => None,
                };
                let t = self.auth(AuthEvent::SignIn(r));
                match user {
                    Some(u) if self.auth.is_signed_in() => {
                        Task::batch([t, self.on_signed_in(Some(u))])
                    }
                    _ => Task::batch([t, operation::focus(Id::new(AUTH_INPUT_ID))]),
                }
            }
            Done::Persisted(r) => match r {
                Err(e) => self.toast(e.user_message()),
                Ok(()) => Task::none(),
            },
            Done::Dialogs(Ok(chats)) => {
                let to_store = chats.clone();
                self.remember_names(chats.iter().filter_map(|c| c.last_message.as_ref()));
                self.dialogs_exhausted = chats.len() < DIALOGS_PAGE;
                self.dialogs_loading = false;
                let exhausted = self.dialogs_exhausted;
                // The open chat may come from a search hit outside the list.
                let keep = self.selected.and_then(|id| self.chats.get(id).cloned());
                self.chats.replace(chats);
                if let Some(c) = keep
                    && self.chats.get(c.id).is_none()
                {
                    self.chats.merge(vec![c]);
                }
                // Pruning is only safe when the first page was the whole list.
                let persist = self.store_bg(move |s| {
                    if exhausted {
                        s.replace_chats(&to_store).map(|_| ())
                    } else {
                        s.upsert_chats(&to_store)
                    }
                });
                Task::batch([persist, self.fetch_avatars(), self.fill_folder()])
            }
            Done::Dialogs(Err(e)) => self.on_error(e),
            Done::MoreDialogs(Ok(chats)) => {
                self.dialogs_loading = false;
                self.dialogs_exhausted = chats.len() < DIALOGS_PAGE;
                self.remember_names(chats.iter().filter_map(|c| c.last_message.as_ref()));
                let to_store = chats.clone();
                self.chats.merge(chats);
                Task::batch([
                    self.store_bg(move |s| s.upsert_chats(&to_store)),
                    self.fetch_avatars(),
                    self.fill_folder(),
                ])
            }
            Done::MoreDialogs(Err(e)) => {
                self.dialogs_loading = false;
                self.on_error(e)
            }
            Done::CacheHistory {
                chat,
                before,
                complete,
                result,
            } => {
                if let Ok(page) = &result {
                    self.remember_names(page);
                }
                let Some(conv) = self.conversation.as_mut().filter(|c| c.chat_id == chat) else {
                    return Task::none();
                };
                // The newest page must not land in a window opened at an older
                // message (a late response from before the jump).
                if conv.detached && before.is_none() {
                    return Task::none();
                }
                let page = result.unwrap_or_default();
                let short = page.len() < PAGE;
                conv.merge(page);
                match before {
                    // Initial open: the network request is already in flight.
                    None => self.fetch_photos(),
                    // Older page proven complete by the contiguity range.
                    Some(_) if complete => {
                        conv.loading_older = false;
                        if short {
                            conv.reached_start = true;
                        }
                        self.fetch_photos()
                    }
                    // Gap or unknown range: ask the network.
                    Some(b) => self.net_history(chat, Some(b)),
                }
            }
            Done::NetHistory {
                chat,
                before,
                result,
            } => {
                let Some(conv) = self.conversation.as_mut().filter(|c| c.chat_id == chat) else {
                    return Task::none();
                };
                if conv.detached && before.is_none() {
                    return Task::none();
                }
                conv.loading_older = false;
                match result {
                    Ok(page) => {
                        if page.len() < PAGE {
                            conv.reached_start = true;
                        }
                        let to_store = page.clone();
                        conv.merge(page);
                        // Scroll to the search hit once its page is in.
                        let mut scroll = Task::none();
                        let mut missing = false;
                        if let Some(target) = conv.jump_to.take() {
                            match conv.index_of(target) {
                                Some(i) => {
                                    let y = state::jump_offset(i, conv.messages().len());
                                    self.at_bottom = false;
                                    scroll = operation::snap_to(
                                        Id::new(HISTORY_ID),
                                        operation::RelativeOffset { x: 0.0, y },
                                    );
                                }
                                None => missing = true,
                            }
                        }
                        let mut tasks = vec![
                            self.store_bg(move |s| {
                                s.record_network_page(chat, before, &to_store, PAGE)
                            }),
                            self.fetch_photos(),
                            scroll,
                        ];
                        if missing {
                            tasks.push(self.toast("That message is no longer available.".into()));
                            tasks.push(self.reattach());
                        }
                        Task::batch(tasks)
                    }
                    Err(e) => self.on_error(e),
                }
            }
            Done::Sent { chat, temp, result } => {
                let ok = result.as_ref().ok().cloned();
                if let Some(conv) = self.conversation.as_mut().filter(|c| c.chat_id == chat) {
                    conv.resolve_pending(temp, ok.clone());
                }
                match result {
                    Ok(m) => {
                        self.chats.set_last(&m);
                        let t = self.fetch_photos();
                        Task::batch([t, self.store_bg(move |s| s.upsert_messages(&[m]))])
                    }
                    Err(e) => self.on_error(e),
                }
            }
            Done::Rollback { old, error } => {
                if let Some(conv) = self
                    .conversation
                    .as_mut()
                    .filter(|c| c.chat_id == old.chat_id)
                {
                    conv.merge(vec![old]);
                }
                self.on_error(error)
            }
            Done::Call(Ok(())) => Task::none(),
            Done::Call(Err(e)) => self.on_error(e),
            Done::SidebarResults { query, result } => {
                // Drop results for a query the user has already changed.
                if query == self.sidebar_query {
                    self.sidebar_messages = result.unwrap_or_default();
                }
                Task::none()
            }
            Done::GlobalResults { query, result } => {
                if query != self.sidebar_query.trim() {
                    return Task::none();
                }
                self.global_searching = false;
                match result {
                    Ok(hits) => {
                        self.remember_names(hits.iter().map(|h| &h.message));
                        // The cache search already shows its own matches.
                        let known: HashSet<(ChatId, MessageId)> = self
                            .sidebar_messages
                            .iter()
                            .map(|m| (m.chat_id, m.id))
                            .collect();
                        self.sidebar_global = hits
                            .into_iter()
                            .filter(|h| !known.contains(&(h.message.chat_id, h.message.id)))
                            .collect();
                    }
                    // Offline or flood-limited: the local results stay.
                    Err(e) => warn!(error = %e, "global search failed"),
                }
                Task::none()
            }
            Done::SearchPage {
                chat,
                version,
                offset,
                result,
            } => self.on_search_page(chat, version, offset, result),
            Done::Members { chat, result } => self.on_members(chat, result),
            Done::NotifyDefaults(result) => match result {
                Ok(d) => {
                    self.chats.set_defaults(d, chrono::Utc::now().timestamp());
                    self.store_bg(move |s| s.save_notify_defaults(&d))
                }
                Err(e) => {
                    // Defaults are best effort: the cached ones stay.
                    warn!(error = %e, "could not load notification defaults");
                    Task::none()
                }
            },
            Done::ChatActionDone { action, result } => self.on_chat_action_done(action, result),
            Done::BlockDone {
                chat,
                blocked,
                result,
            } => self.on_block_done(chat, blocked, result),
            Done::RollbackMany { old, error } => {
                if let Some(conv) = self
                    .conversation
                    .as_mut()
                    .filter(|c| old.first().is_some_and(|m| m.chat_id == c.chat_id))
                {
                    conv.merge(old);
                }
                self.on_error(error)
            }
            Done::FileStat { path, result } => match result {
                Ok((_, true)) => self.toast("Folders can't be sent. Drop a file instead.".into()),
                Ok((size, false)) => {
                    self.drop_pending();
                    self.pending_file = Some(PendingFile {
                        temp: path.starts_with(self.config.paste_dir()),
                        name: path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "file".into()),
                        kind: UploadKind::for_path(&path),
                        size,
                        path,
                    });
                    operation::focus(Id::new(COMPOSER_ID))
                }
                Err(e) => self.on_error(e),
            },
            Done::SavePicked {
                chat,
                id,
                path: Some(path),
            } => {
                let Some(api) = self.api.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { api.download_media(chat, id, path).await },
                    |r| Message::Done(Done::MediaSaved(r)),
                )
            }
            Done::SavePicked { path: None, .. } => Task::none(),
            Done::Pasted(clip) => self.on_pasted(clip),
            Done::PasteSaved(Ok(path)) => self.update_ui(Ui::FilePicked(Some(path))),
            Done::PasteSaved(Err(e)) => self.on_error(e),
            Done::ImageCopied(Ok(())) => self.toast("Image copied.".into()),
            Done::ImageCopied(Err(e)) => self.on_error(e),
            Done::PeerInfo { chat, result } => {
                // The ⋮ menu needs `blocked` even when the panel is closed.
                if let Ok(info) = &result {
                    self.blocked.insert(chat, info.blocked);
                }
                let Some(panel) = self.info.as_mut().filter(|p| p.chat == chat) else {
                    return Task::none();
                };
                match result {
                    Ok(info) => panel.info = Some(info),
                    Err(e) => {
                        // The panel still shows the name and the tabs.
                        warn!(error = %e, "could not load chat info");
                        panel.info_failed = true;
                    }
                }
                Task::none()
            }
            Done::SharedPage {
                chat,
                kind,
                offset,
                result,
            } => {
                let Some(panel) = self.info.as_mut().filter(|p| p.chat == chat) else {
                    return Task::none();
                };
                match result {
                    Ok(page) => {
                        panel.tab_mut(kind).apply(offset, page);
                        self.request_thumbs()
                    }
                    Err(e) => {
                        if e.is_session_revoked() {
                            return self.session_revoked();
                        }
                        warn!(error = %e, "could not load shared content");
                        panel.tab_mut(kind).fail(offset);
                        Task::none()
                    }
                }
            }
            Done::Thumb { key, result } => match result {
                Ok(bytes) if state::photo_is_acceptable(&bytes) => {
                    // Decode and crop to a fixed tile on a blocking thread:
                    // the UI thread only ever sees small RGBA handles.
                    let side = info::thumb_side(self.scale);
                    Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                crate::media::ThumbImage::decode(&bytes, side)
                                    .ok_or_else(|| Error::Protocol("thumbnail rejected".into()))
                            })
                            .await
                            .map_err(|_| Error::Cancelled)?
                        },
                        move |result| Message::Done(Done::ThumbReady { key, result }),
                    )
                }
                Ok(_) => {
                    self.thumb_q.finish(key, false);
                    self.pump_thumbs()
                }
                Err(e) => {
                    warn!(error = %e, "thumbnail unavailable");
                    self.thumb_q.finish(key, false);
                    self.pump_thumbs()
                }
            },
            Done::ThumbReady { key, result } => {
                let ok = match result {
                    Ok(t) => {
                        // Only while the panel still shows that chat.
                        if self.info.as_ref().is_some_and(|p| p.chat == key.0) {
                            let cost = t.cost();
                            // One handle per thumbnail, kept until evicted:
                            // `view` clones it, so its id never changes.
                            let handle = image::Handle::from_rgba(t.width, t.height, t.rgba);
                            self.thumbs.insert(key, handle, cost);
                        }
                        true
                    }
                    Err(e) => {
                        warn!(error = %e, "thumbnail not usable");
                        false
                    }
                };
                self.thumb_q.finish(key, ok);
                self.pump_thumbs()
            }
            Done::MuteDone {
                chat,
                previous,
                result,
            } => match result {
                Ok(()) => Task::none(),
                Err(e) => {
                    self.chats
                        .set_notify(chat, previous, chrono::Utc::now().timestamp());
                    let persist = self.store_bg(move |s| s.set_notify(chat, &previous));
                    Task::batch([persist, self.on_error(e)])
                }
            },
            done @ (Done::UpdateMethod(_)
            | Done::UpdateChecked { .. }
            | Done::UpdateProgress(_)
            | Done::UpdateInstalled(_)) => self.on_update_done(done),
            Done::LinkOpened(Ok(())) => Task::none(),
            Done::LinkOpened(Err(e)) => {
                warn!(error = %e, "could not open link");
                self.toast("Could not open the link.".into())
            }
            Done::Folders(Ok(list)) => {
                self.folders.set_list(list);
                Task::batch([self.persist_folders(), self.fill_folder()])
            }
            Done::Folders(Err(e)) => {
                // Folders are optional chrome: log, never toast.
                warn!(error = %e, "could not load chat folders");
                Task::none()
            }
            Done::FolderSaved { folder, result } => match result {
                Ok(()) => {
                    self.folders.upsert(folder);
                    self.folder_editor = None;
                    Task::batch([self.persist_folders(), self.fill_folder()])
                }
                Err(e) => {
                    if e.is_session_revoked() {
                        return self.session_revoked();
                    }
                    warn!(error = %e, "could not save folder");
                    if let Some(ed) = &mut self.folder_editor {
                        ed.saving = false;
                        ed.error = Some(e.user_message());
                    }
                    Task::none()
                }
            },
            Done::FolderOp { before, result } => match result {
                Ok(()) => Task::none(),
                Err(e) => {
                    self.folders.set_list(before);
                    let persist = self.persist_folders();
                    Task::batch([persist, self.on_error(e)])
                }
            },
            Done::MediaSaved(Ok(())) => self.toast("File saved.".into()),
            Done::MediaSaved(Err(e)) => self.on_error(e),
            Done::Photo { key, result } => {
                self.photo_loading.remove(&key);
                if let Ok(bytes) = result {
                    if !state::photo_is_acceptable(&bytes) {
                        warn!(bytes = bytes.len(), "photo rejected by size/pixel cap");
                        return Task::none();
                    }
                    // Budget by the decoded size when known.
                    let size = state::image_dimensions(&bytes)
                        .map_or(bytes.len() * 6, |(w, h)| (w as usize) * (h as usize) * 4);
                    self.photos
                        .insert(key, image::Handle::from_bytes(bytes), size);
                }
                Task::none()
            }
            Done::Avatar {
                chat,
                photo_id,
                result,
            } => {
                let key = (chat, photo_id);
                self.avatar_loading.remove(&key);
                match result {
                    Ok(img) => {
                        let size = (img.width as usize) * (img.height as usize) * 4;
                        let handle =
                            image::Handle::from_rgba(img.width, img.height, (*img.rgba).clone());
                        self.avatars
                            .insert(chat, (photo_id, handle), size.max(AVATAR_COST / 4));
                    }
                    // Not available right now (offline, disk miss): the next
                    // trigger (chat list refresh, opening a chat) retries.
                    Err(Error::Cancelled) => return Task::none(),
                    Err(e) => {
                        warn!(error = %e, "avatar unavailable");
                        self.avatar_failed.insert(key);
                    }
                }
                self.fetch_avatars()
            }
            Done::FileReady(Ok(path)) => Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || crate::desktop::open_file(&path))
                        .await
                        .map_err(|_| Error::Cancelled)?
                },
                |r| Message::Done(Done::FileOpened(r)),
            ),
            Done::FileReady(Err(e)) => self.on_error(e),
            Done::FileOpened(Ok(())) => Task::none(),
            Done::FileOpened(Err(e)) => {
                warn!(error = %e, "could not open file");
                self.toast("No application could open this file. Use “Save as…”.".into())
            }
            Done::LoggedOut => {
                self.reset_session();
                self.auth = AuthState::Phone {
                    input: String::new(),
                    error: None,
                    busy: false,
                };
                self.rebuild_api()
            }
            Done::Screenshot(shot) => screenshot::save(&self.config, shot),
            Done::ScreenshotSaved => iced::exit(),
            Done::TrayReady(Ok(handle)) => {
                info!("tray icon registered");
                self.tray = Some(handle);
                // Whatever changed while registering goes out on the next sync.
                self.tray_sent = None;
                let action = self.window.tray_ready(true);
                self.apply_window(action)
            }
            Done::TrayReady(Err(reason)) => {
                info!(%reason, "no tray: closing the window quits");
                let action = self.window.tray_ready(false);
                self.apply_window(action)
            }
            Done::WindowOpened(id) => {
                window::scale_factor(id).map(|f| Message::Ui(Ui::Rescaled(f)))
            }
            Done::AutostartSet(Ok(on)) => {
                self.autostart = on;
                Task::none()
            }
            Done::AutostartSet(Err(e)) => {
                warn!(error = %e, "could not change autostart");
                self.autostart = crate::autostart::system_is_enabled();
                self.toast("Could not change the launch-at-login setting".into())
            }
        }
    }

    /// Per-call failures become a toast; they never change the connection
    /// state (only the update stream and restore do), so a single failed
    /// request cannot leave a stale "Waiting for network…" banner.
    fn on_error(&mut self, e: Error) -> Task<Message> {
        if e.is_session_revoked() {
            return self.session_revoked();
        }
        warn!(error = %e, "operation failed");
        self.toast(e.user_message())
    }
}

/// Disk cache first, then the network. Always validates the bytes before
/// they reach the image decoder. `Err(Cancelled)` means "not now".
pub async fn load_avatar(
    api: Option<ApiHandle>,
    disk: Option<AvatarDisk>,
    (chat, photo_id): (ChatId, i64),
) -> Result<Vec<u8>, Error> {
    if let Some(d) = disk.clone() {
        let cached = tokio::task::spawn_blocking(move || d.read(chat, photo_id))
            .await
            .map_err(|_| Error::Cancelled)?;
        if let Some(bytes) = cached.filter(|b| state::avatar_is_acceptable(b)) {
            return Ok(bytes);
        }
    }
    let api = api.ok_or(Error::Cancelled)?;
    let bytes = api.download_avatar(chat, photo_id).await?;
    if !state::avatar_is_acceptable(&bytes) {
        return Err(Error::Protocol("avatar rejected".into()));
    }
    if let Some(d) = disk {
        let b = bytes.clone();
        let _ = tokio::task::spawn_blocking(move || d.write(chat, photo_id, &b)).await;
    }
    Ok(bytes)
}

fn open_memory_store() -> Store {
    match Store::open_in_memory() {
        Ok(s) => s,
        Err(e) => {
            // In-memory SQLite failing means the process is unusable anyway.
            eprintln!("fatal: cannot open in-memory store: {e}");
            std::process::exit(1)
        }
    }
}
