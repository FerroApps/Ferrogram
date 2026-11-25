//! Mock data and a fake Telegram backend, used by tests and `--mock` mode.
//!
//! [`FakeTelegram`] implements [`TelegramApi`] entirely in memory, so the
//! whole app (login state machine, chat list, history paging, sending,
//! edits, deletes, reactions, search, notifications) can be exercised
//! without an account or network.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use secrecy::{ExposeSecret, SecretString};

use crate::auth::SignInOutcome;
use crate::error::{AuthError, Error, Phone, Result};
use crate::model::{
    CHANNEL_OFFSET, Chat, ChatId, ChatKind, Folder, GlobalHit, Media, Member, Message, MessageId,
    NotifyDefaults, NotifySettings, NotifyTarget, PeerInfo, Presence, Reaction, SharedBody,
    SharedItem, SharedKind, SharedPage, TypingAction, User,
};
use crate::net::{Event, EventHub, TelegramApi, UploadKind};

/// Fixed "now" so screenshots and tests are deterministic (2026-11-18 14:32 UTC).
pub const MOCK_NOW: i64 = 1_795_012_320;
pub const SELF_ID: i64 = 100;
pub const ALICE: ChatId = 201;
pub const ECHO_BOT: ChatId = 206;
pub const RUST_GROUP: ChatId = -301;
/// Group whose title, senders and messages carry the tricky emoji sequences.
pub const EMOJI_GROUP: ChatId = -305;
pub const DEV_CHANNEL: ChatId = -CHANNEL_OFFSET - 401;
pub const NEWS_CHANNEL: ChatId = -CHANNEL_OFFSET - 402;
/// Login code that leads to the 2FA screen.
pub const CODE_2FA: &str = "12345";
/// Login code that signs in without 2FA.
pub const CODE_PLAIN: &str = "22222";
pub const PASSWORD: &str = "hunter2";

pub fn me() -> User {
    User {
        id: SELF_ID,
        name: "Sam Rivera".into(),
        username: Some("samr".into()),
    }
}

const MIN: i64 = 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;

fn msg(chat: ChatId, id: MessageId, out: bool, date: i64, text: &str) -> Message {
    let mut m = Message::text(chat, id, out, date, text);
    if !out {
        m.sender_id = Some(chat);
    } else {
        m.sender_id = Some(SELF_ID);
    }
    m
}

fn group_msg(chat: ChatId, id: MessageId, who: (i64, &str), date: i64, text: &str) -> Message {
    let mut m = Message::text(chat, id, who.0 == SELF_ID, date, text);
    m.sender_id = Some(who.0);
    if who.0 != SELF_ID {
        m.sender_name = Some(who.1.into());
    }
    m
}

const FILLER: &[&str] = &[
    "Did you see the release notes?",
    "Yes! The new scheduler looks great",
    "I'll try it on the laptop tonight",
    "Haha, that's exactly what I said",
    "Let me check and get back to you",
    "Sounds good 👍",
    "Can you send me the link again?",
    "Sure, one sec",
    "On my way",
    "Perfect, thanks!",
    "What time works for you?",
    "After lunch is fine",
];

/// Alice: a long private history (for infinite scroll) ending in a scripted
/// conversation with a photo, a file, a reply, an edit and reactions.
fn alice_history() -> Vec<Message> {
    let mut v = Vec::new();
    let mut id = 1;
    let start = MOCK_NOW - 6 * DAY;
    for i in 0..140 {
        let out = i % 3 == 1;
        let t = start + i64::from(i) * 50 * MIN;
        v.push(msg(ALICE, id, out, t, FILLER[i as usize % FILLER.len()]));
        id += 1;
    }
    let base = MOCK_NOW - 2 * HOUR;
    let mut push = |out: bool, dt: i64, text: &str, f: &mut dyn FnMut(&mut Message)| {
        let mut m = msg(ALICE, id, out, base + dt, text);
        f(&mut m);
        v.push(m);
        id += 1;
    };
    push(
        false,
        0,
        "Hey Sam! Are we still on for the hike on Saturday? 🏔️",
        &mut |_| {},
    );
    push(
        true,
        2 * MIN,
        "Absolutely! I already packed my bag",
        &mut |_| {},
    );
    push(false, 3 * MIN, "", &mut |m| {
        m.media = Some(Media::Photo {
            id: 9001,
            width: 1280,
            height: 853,
        });
        m.text = "The view from the top last time 😍".into();
    });
    push(true, 5 * MIN, "Wow, that's stunning", &mut |m| {
        m.reactions = vec![Reaction {
            emoji: "❤️".into(),
            count: 1,
            chosen: false,
        }];
    });
    push(false, 9 * MIN, "", &mut |m| {
        m.media = Some(Media::File {
            name: "trail-map.pdf".into(),
            size: 2_457_600,
            mime: "application/pdf".into(),
        });
    });
    push(
        false,
        10 * MIN,
        "Here's the trail map. It's about 14 km round trip",
        &mut |_| {},
    );
    push(
        true,
        12 * MIN,
        "Great, I'll print it out. Should we start at 8?",
        &mut |m| {
            m.reply_to = Some(145);
            m.edited = true;
        },
    );
    push(
        false,
        15 * MIN,
        "8 is perfect. I'll bring coffee ☕",
        &mut |m| {
            m.reactions = vec![
                Reaction {
                    emoji: "👍".into(),
                    count: 1,
                    chosen: true,
                },
                Reaction {
                    emoji: "🔥".into(),
                    count: 1,
                    chosen: false,
                },
            ];
        },
    );
    push(true, 16 * MIN, "You're the best!", &mut |_| {});
    push(false, 118 * MIN, "See you on Saturday then!", &mut |_| {});
    v
}

fn chats_and_history() -> (Vec<Chat>, BTreeMap<ChatId, Vec<Message>>) {
    let mut chats = Vec::new();
    let mut hist: BTreeMap<ChatId, Vec<Message>> = BTreeMap::new();

    let mut add = |id: ChatId,
                   kind: ChatKind,
                   title: &str,
                   unread: u32,
                   muted: bool,
                   pinned: bool,
                   msgs: Vec<Message>| {
        let mut c = Chat::new(id, kind, title);
        c.unread_count = unread;
        c.muted = muted;
        if muted {
            c.notify.mute_until = Some(crate::model::MUTE_FOREVER);
        }
        c.pinned = pinned;
        c.last_message = msgs.last().cloned();
        c.last_date = c.last_message.as_ref().map_or(0, |m| m.date);
        let max_out = msgs.iter().filter(|m| m.outgoing).map(|m| m.id).max();
        c.read_outbox_max_id = max_out.unwrap_or(0);
        c.read_inbox_max_id = msgs
            .len()
            .saturating_sub(unread as usize)
            .try_into()
            .unwrap_or(0);
        chats.push(c);
        hist.insert(id, msgs);
    };

    add(
        SELF_ID,
        ChatKind::SavedMessages,
        "Saved Messages",
        0,
        false,
        true,
        vec![msg(
            SELF_ID,
            1,
            true,
            MOCK_NOW - 3 * DAY,
            "Wi-Fi at the cabin: ask Alice",
        )],
    );
    add(
        ALICE,
        ChatKind::User,
        "Alice Johnson",
        1,
        false,
        false,
        alice_history(),
    );

    let ana = (311, "Ana Torres");
    let leo = (312, "Leo Park");
    add(
        RUST_GROUP,
        ChatKind::Group,
        "Rustaceans Madrid",
        12,
        false,
        false,
        vec![
            group_msg(
                RUST_GROUP,
                1,
                ana,
                MOCK_NOW - 5 * HOUR,
                "Meetup next Thursday at the usual place",
            ),
            group_msg(
                RUST_GROUP,
                2,
                leo,
                MOCK_NOW - 4 * HOUR,
                "Who's giving the async talk?",
            ),
            group_msg(
                RUST_GROUP,
                3,
                (SELF_ID, "Sam"),
                MOCK_NOW - 3 * HOUR,
                "I can do a short one on iced 0.14",
            ),
            group_msg(
                RUST_GROUP,
                4,
                ana,
                MOCK_NOW - 25 * MIN,
                "Leo: has anyone tried the new borrow checker?",
            ),
        ],
    );
    let mia = (321, "Mia \u{1F469}\u{200D}\u{1F4BB}");
    let kai = (322, "Kai \u{1F1EF}\u{1F1F5}");
    add(
        EMOJI_GROUP,
        ChatKind::Group,
        "Emoji lab \u{1F9EA}\u{1F1EA}\u{1F1F8}",
        2,
        false,
        false,
        vec![
            group_msg(
                EMOJI_GROUP,
                1,
                mia,
                MOCK_NOW - 50 * MIN,
                "Keycaps: 1\u{FE0F}\u{20E3} 2\u{FE0F}\u{20E3} #\u{FE0F}\u{20E3} *\u{FE0F}\u{20E3} but plain 12 # stay text",
            ),
            group_msg(
                EMOJI_GROUP,
                2,
                kai,
                MOCK_NOW - 40 * MIN,
                "Flags \u{1F1EA}\u{1F1F8}\u{1F1EF}\u{1F1F5}\u{1F1E9}\u{1F1EA} \u{1F3F3}\u{FE0F}\u{200D}\u{1F308} and a family \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
            ),
            group_msg(
                EMOJI_GROUP,
                3,
                mia,
                MOCK_NOW - 30 * MIN,
                "Skin tones \u{1F44B}\u{1F3FB}\u{1F44B}\u{1F3FD}\u{1F44B}\u{1F3FF} \u{1F9D1}\u{1F3FD}\u{200D}\u{2695}\u{FE0F}",
            ),
            group_msg(
                EMOJI_GROUP,
                4,
                kai,
                MOCK_NOW - 20 * MIN,
                "Marks: \u{A9}\u{FE0F} \u{AE}\u{FE0F} \u{2122}\u{FE0F} \u{203C}\u{FE0F} \u{3030}\u{FE0F} \u{3297}\u{FE0F} vs plain \u{A9} \u{AE} \u{2122}",
            ),
        ],
    );
    add(
        DEV_CHANNEL,
        ChatKind::Channel,
        "Ferrogram Dev",
        3,
        false,
        false,
        vec![group_msg(
            DEV_CHANNEL,
            1,
            (0, "Ferrogram Dev"),
            MOCK_NOW - 47 * MIN,
            "Build 0.1.0 is out: reactions, search and notifications",
        )],
    );
    add(
        202,
        ChatKind::User,
        "Bob Martinez",
        0,
        false,
        false,
        vec![
            msg(
                202,
                1,
                false,
                MOCK_NOW - DAY - 2 * HOUR,
                "Thanks for the help yesterday",
            ),
            msg(202, 2, true, MOCK_NOW - DAY - HOUR, "Anytime! 🙌"),
        ],
    );
    add(
        ECHO_BOT,
        ChatKind::Bot,
        "Echo Bot",
        0,
        false,
        false,
        vec![msg(
            ECHO_BOT,
            1,
            false,
            MOCK_NOW - 2 * DAY,
            "Send me anything and I'll repeat it.",
        )],
    );
    add(
        -302,
        ChatKind::Group,
        "Family",
        0,
        false,
        false,
        vec![group_msg(
            -302,
            1,
            (313, "Mom"),
            MOCK_NOW - 2 * DAY - HOUR,
            "Sunday lunch at 2?",
        )],
    );
    add(
        203,
        ChatKind::User,
        "Carla Gómez",
        2,
        false,
        false,
        vec![
            msg(
                203,
                1,
                false,
                MOCK_NOW - 3 * DAY,
                "Are you coming to the concert?",
            ),
            msg(
                203,
                2,
                false,
                MOCK_NOW - 3 * DAY + MIN,
                "I have an extra ticket 🎟️",
            ),
        ],
    );
    add(
        NEWS_CHANNEL,
        ChatKind::Channel,
        "Tech News Daily",
        128,
        true,
        false,
        vec![group_msg(
            NEWS_CHANNEL,
            1,
            (0, "Tech News Daily"),
            MOCK_NOW - 4 * DAY,
            "Wayland adoption hits a new record",
        )],
    );
    add(
        -303,
        ChatKind::Group,
        "Weekend Hiking",
        0,
        true,
        false,
        vec![group_msg(
            -303,
            1,
            (314, "Marta"),
            MOCK_NOW - 5 * DAY,
            "Photo",
        )],
    );
    add(
        204,
        ChatKind::User,
        "David Chen",
        0,
        false,
        false,
        vec![msg(
            204,
            1,
            true,
            MOCK_NOW - 6 * DAY,
            "Let's catch up next week",
        )],
    );
    add(
        -304,
        ChatKind::Group,
        "Book Club",
        0,
        false,
        false,
        vec![group_msg(
            -304,
            1,
            (315, "Irene"),
            MOCK_NOW - 9 * DAY,
            "Next book: The Left Hand of Darkness",
        )],
    );
    // A few chats have profile photos, like a real account; the rest keep
    // their initials. Alice is online, the bot is a bot.
    for c in &mut chats {
        if [ALICE, RUST_GROUP, DEV_CHANNEL, -303].contains(&c.id) {
            c.photo_id = Some(7_000 + c.id.rem_euclid(1000));
        }
        if c.id == -304 {
            c.draft = Some(crate::model::Draft {
                text: "I'll bring the cookies".into(),
                reply_to: None,
                date: MOCK_NOW - 600,
            });
        }
        // Everybody but Carla is in the mock account's contacts.
        c.contact = c.kind == ChatKind::User && c.title != "Carla Gómez";
        if c.id == ALICE {
            c.presence = Presence::Online {
                until: MOCK_NOW + 5 * MIN,
            };
        }
    }
    if let Some(v) = hist.get_mut(&-303)
        && let Some(m) = v.last_mut()
    {
        m.text.clear();
        m.media = Some(Media::Photo {
            id: 9002,
            width: 800,
            height: 600,
        });
    }
    if let Some(c) = chats.iter_mut().find(|c| c.id == -303) {
        c.last_message = hist.get(&-303).and_then(|v| v.last().cloned());
    }
    (chats, hist)
}

/// Chats exactly as the fake backend would return them.
pub fn mock_chats() -> Vec<Chat> {
    chats_and_history().0
}

pub fn mock_history(chat: ChatId) -> Vec<Message> {
    chats_and_history().1.remove(&chat).unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct FakeOptions {
    /// Start already logged in.
    pub logged_in: bool,
    /// Echo Bot answers and peers "read" our messages after a delay.
    pub live_simulation: bool,
    pub latency: Duration,
    /// Extra filler contacts (older than every scripted chat), to exercise
    /// chat-list paging.
    pub extra_chats: usize,
}

impl Default for FakeOptions {
    fn default() -> Self {
        Self {
            logged_in: false,
            live_simulation: true,
            latency: Duration::from_millis(0),
            extra_chats: 0,
        }
    }
}

struct State {
    authorized: bool,
    code_sent: bool,
    password_pending: bool,
    chats: Vec<Chat>,
    history: BTreeMap<ChatId, Vec<Message>>,
    calls: Vec<String>,
    next_id: MessageId,
    /// How many dialogs the paging cursor has handed out.
    dialog_cursor: usize,
    folders: Vec<Folder>,
    notify_defaults: NotifyDefaults,
    blocked: std::collections::HashSet<ChatId>,
}

pub struct FakeTelegram {
    state: Mutex<State>,
    opts: FakeOptions,
    events: Arc<EventHub>,
    /// Fake call server + remote peer (real engine on loopback).
    phone: crate::mock_call::FakeCalls,
}

impl FakeTelegram {
    pub fn new(opts: FakeOptions) -> Arc<Self> {
        let events = Arc::new(EventHub::new());
        let (mut chats, mut history) = chats_and_history();
        for i in 0..opts.extra_chats {
            let id = 1_000 + i as i64;
            let mut c = Chat::new(id, ChatKind::User, format!("Contact {i:03}"));
            let m = msg(
                id,
                1,
                false,
                MOCK_NOW - 30 * DAY - i as i64 * MIN,
                "Older conversation",
            );
            c.last_date = m.date;
            c.last_message = Some(m.clone());
            history.insert(id, vec![m]);
            chats.push(c);
        }
        Arc::new(Self {
            state: Mutex::new(State {
                authorized: opts.logged_in,
                code_sent: false,
                password_pending: false,
                chats,
                history,
                calls: Vec::new(),
                next_id: 10_000,
                dialog_cursor: 0,
                folders: mock_folders(),
                notify_defaults: NotifyDefaults::default(),
                blocked: std::collections::HashSet::new(),
            }),
            opts,
            phone: crate::mock_call::FakeCalls::new(me().id, Arc::clone(&events)),
            events,
        })
    }

    /// The fake call server and peer (tests, screenshots).
    pub fn phone(&self) -> &crate::mock_call::FakeCalls {
        &self.phone
    }

    fn st(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| Error::Net("fake poisoned".into()))
    }

    /// Names of the API calls made so far (for integration tests).
    pub fn calls(&self) -> Vec<String> {
        self.st().map(|s| s.calls.clone()).unwrap_or_default()
    }

    /// Inject a server-side event, as if it came from the update stream.
    pub fn push_event(&self, ev: Event) {
        if let Ok(mut s) = self.st()
            && let Event::NewMessage(m) = &ev
        {
            s.history.entry(m.chat_id).or_default().push(m.clone());
        }
        self.events.emit(ev);
    }

    /// Set the account defaults `notify_defaults` will return.
    pub fn set_notify_defaults(&self, d: NotifyDefaults) {
        if let Ok(mut s) = self.st() {
            s.notify_defaults = d;
        }
    }

    /// Whether `chat` is blocked on the fake server.
    pub fn is_blocked(&self, chat: ChatId) -> bool {
        self.st().is_ok_and(|s| s.blocked.contains(&chat))
    }

    /// The fake server's notification settings of `chat`.
    pub fn notify_of(&self, chat: ChatId) -> Option<NotifySettings> {
        self.st()
            .ok()?
            .chats
            .iter()
            .find(|c| c.id == chat)
            .map(|c| c.notify)
    }

    /// Whether the fake server still has the chat.
    pub fn has_chat(&self, chat: ChatId) -> bool {
        self.st()
            .is_ok_and(|s| s.chats.iter().any(|c| c.id == chat))
    }

    /// Number of messages the fake server holds for `chat`.
    pub fn history_len(&self, chat: ChatId) -> usize {
        self.st()
            .map(|s| s.history.get(&chat).map_or(0, Vec::len))
            .unwrap_or(0)
    }

    async fn call(&self, name: &str) -> Result<()> {
        if !self.opts.latency.is_zero() {
            tokio::time::sleep(self.opts.latency).await;
        }
        self.st()?.calls.push(name.to_owned());
        Ok(())
    }

    fn require_auth(&self) -> Result<()> {
        if self.st()?.authorized {
            Ok(())
        } else {
            Err(Error::Auth(AuthError::SessionRevoked))
        }
    }

    /// The next `limit` dialogs in list order (pinned, then newest first).
    fn dialog_slice(&self, limit: usize) -> Result<Vec<Chat>> {
        let mut s = self.st()?;
        let mut chats = s.chats.clone();
        for c in &mut chats {
            if let Some(last) = s.history.get(&c.id).and_then(|h| h.last()) {
                c.last_date = last.date;
                c.last_message = Some(last.clone());
            }
        }
        crate::app::state::sort_chats(&mut chats);
        let from = s.dialog_cursor.min(chats.len());
        let page: Vec<Chat> = chats.into_iter().skip(from).take(limit).collect();
        s.dialog_cursor = from + page.len();
        Ok(page)
    }

    fn append_outgoing(&self, mut m: Message) -> Result<Message> {
        let mut s = self.st()?;
        s.next_id += 1;
        m.id = s.next_id;
        s.history.entry(m.chat_id).or_default().push(m.clone());
        Ok(m)
    }

    fn simulate_after_send(&self, sent: &Message) {
        if !self.opts.live_simulation {
            return;
        }
        let tx = Arc::clone(&self.events);
        let chat = sent.chat_id;
        let id = sent.id;
        let text = sent.text.clone();
        let echo_id = self.st().map(|mut s| {
            s.next_id += 1;
            s.next_id
        });
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(900)).await;
            tx.emit(Event::ReadOutbox { chat, max_id: id });
            if chat == ECHO_BOT
                && let Ok(echo_id) = echo_id
            {
                tx.emit(Event::Typing {
                    chat: ECHO_BOT,
                    user: ECHO_BOT,
                    action: Some(TypingAction::Typing),
                });
                tokio::time::sleep(Duration::from_millis(1200)).await;
                let now = chrono::Utc::now().timestamp();
                let reply = if text.is_empty() {
                    "Nice file!".to_owned()
                } else {
                    text
                };
                let mut m = msg(ECHO_BOT, echo_id, false, now, &reply);
                m.reply_to = Some(id);
                tx.emit(Event::NewMessage(m));
            }
        });
    }
}

#[async_trait]
impl TelegramApi for FakeTelegram {
    async fn restore(&self) -> Result<Option<User>> {
        self.call("restore").await?;
        Ok(self.st()?.authorized.then(me))
    }

    async fn request_code(&self, phone: Phone) -> Result<()> {
        self.call("request_code").await?;
        if !phone.is_plausible() || phone.normalized().starts_with("+000") {
            return Err(Error::Auth(AuthError::InvalidPhone));
        }
        self.st()?.code_sent = true;
        Ok(())
    }

    async fn sign_in(&self, code: String) -> Result<SignInOutcome> {
        self.call("sign_in").await?;
        let mut s = self.st()?;
        if !s.code_sent {
            return Err(Error::Auth(AuthError::InvalidCode));
        }
        match code.as_str() {
            CODE_2FA => {
                s.password_pending = true;
                Ok(SignInOutcome::PasswordRequired {
                    hint: Some("usual one".into()),
                })
            }
            CODE_PLAIN => {
                s.authorized = true;
                Ok(SignInOutcome::SignedIn(me()))
            }
            _ => Err(Error::Auth(AuthError::InvalidCode)),
        }
    }

    async fn check_password(&self, password: SecretString) -> Result<SignInOutcome> {
        self.call("check_password").await?;
        let mut s = self.st()?;
        if s.password_pending && password.expose_secret() == PASSWORD {
            s.password_pending = false;
            s.authorized = true;
            Ok(SignInOutcome::SignedIn(me()))
        } else {
            Err(Error::Auth(AuthError::InvalidPassword))
        }
    }

    async fn persist_session(&self) -> Result<()> {
        self.call("persist_session").await
    }

    async fn log_out(&self) -> Result<()> {
        self.call("log_out").await?;
        self.st()?.authorized = false;
        Ok(())
    }

    async fn dialogs(&self, limit: usize) -> Result<Vec<Chat>> {
        self.call("dialogs").await?;
        self.require_auth()?;
        self.st()?.dialog_cursor = 0;
        self.dialog_slice(limit)
    }

    async fn more_dialogs(&self, limit: usize) -> Result<Vec<Chat>> {
        self.call("more_dialogs").await?;
        self.require_auth()?;
        self.dialog_slice(limit)
    }

    async fn history(
        &self,
        chat: ChatId,
        before: Option<MessageId>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        self.call("history").await?;
        self.require_auth()?;
        let s = self.st()?;
        let all = s.history.get(&chat).cloned().unwrap_or_default();
        let older: Vec<_> = all
            .into_iter()
            .filter(|m| before.is_none_or(|b| m.id < b))
            .collect();
        let skip = older.len().saturating_sub(limit);
        Ok(older.into_iter().skip(skip).collect())
    }

    async fn send_text(
        &self,
        chat: ChatId,
        text: String,
        reply_to: Option<MessageId>,
    ) -> Result<Message> {
        self.call("send_text").await?;
        self.require_auth()?;
        let mut m = msg(chat, 0, true, chrono::Utc::now().timestamp(), &text);
        m.reply_to = reply_to;
        let sent = self.append_outgoing(m)?;
        self.simulate_after_send(&sent);
        Ok(sent)
    }

    async fn send_file(
        &self,
        chat: ChatId,
        path: PathBuf,
        kind: UploadKind,
        caption: String,
        reply_to: Option<MessageId>,
    ) -> Result<Message> {
        self.call("send_file").await?;
        self.require_auth()?;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let mut m = msg(chat, 0, true, chrono::Utc::now().timestamp(), &caption);
        m.reply_to = reply_to;
        m.media = Some(match kind {
            UploadKind::Photo => Media::Photo {
                id: 50_000 + i64::try_from(size % 1000).unwrap_or(0),
                width: 640,
                height: 420,
            },
            UploadKind::Document => Media::File {
                name,
                size,
                mime: "application/octet-stream".into(),
            },
        });
        let sent = self.append_outgoing(m)?;
        self.simulate_after_send(&sent);
        Ok(sent)
    }

    async fn edit_message(&self, chat: ChatId, id: MessageId, text: String) -> Result<()> {
        self.call("edit_message").await?;
        let edited = {
            let mut s = self.st()?;
            let m = s
                .history
                .get_mut(&chat)
                .and_then(|h| h.iter_mut().find(|m| m.id == id))
                .ok_or_else(|| Error::Protocol("MESSAGE_ID_INVALID".into()))?;
            m.text = text;
            m.edited = true;
            m.clone()
        };
        self.events.emit(Event::MessageEdited(edited));
        Ok(())
    }

    async fn delete_messages(&self, chat: ChatId, ids: Vec<MessageId>) -> Result<()> {
        self.call("delete_messages").await?;
        if let Some(h) = self.st()?.history.get_mut(&chat) {
            h.retain(|m| !ids.contains(&m.id));
        }
        let channel = chat < -CHANNEL_OFFSET;
        self.events.emit(Event::MessagesDeleted {
            chat: channel.then_some(chat),
            ids,
        });
        Ok(())
    }

    async fn react(&self, chat: ChatId, id: MessageId, emoji: Option<String>) -> Result<()> {
        self.call("react").await?;
        let reactions = {
            let mut s = self.st()?;
            let m = s
                .history
                .get_mut(&chat)
                .and_then(|h| h.iter_mut().find(|m| m.id == id))
                .ok_or_else(|| Error::Protocol("MESSAGE_ID_INVALID".into()))?;
            crate::app::state::toggle_reaction(&mut m.reactions, emoji.as_deref());
            m.reactions.clone()
        };
        self.events.emit(Event::Reactions {
            chat,
            id,
            reactions,
        });
        Ok(())
    }

    async fn mark_read(&self, chat: ChatId) -> Result<()> {
        self.call("mark_read").await?;
        let mut s = self.st()?;
        if let Some(c) = s.chats.iter_mut().find(|c| c.id == chat) {
            c.unread_count = 0;
        }
        Ok(())
    }

    async fn search(
        &self,
        chat: ChatId,
        query: String,
        from: Option<i64>,
        offset_id: MessageId,
        limit: usize,
    ) -> Result<Vec<Message>> {
        self.call("search").await?;
        let q = query.to_lowercase();
        let s = self.st()?;
        let mut out: Vec<_> = s
            .history
            .get(&chat)
            .map(|h| {
                h.iter()
                    .filter(|m| !m.service && m.text.to_lowercase().contains(&q))
                    .filter(|m| from.is_none_or(|f| m.sender_id == Some(f)))
                    .filter(|m| offset_id <= 0 || m.id < offset_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        out.reverse();
        out.truncate(limit);
        Ok(out)
    }

    async fn members(&self, chat: ChatId, limit: usize) -> Result<Vec<Member>> {
        self.call("members").await?;
        let s = self.st()?;
        let mut out: Vec<Member> = Vec::new();
        if chat < 0 {
            for m in s.history.get(&chat).into_iter().flatten() {
                if let (Some(id), Some(name)) = (m.sender_id, &m.sender_name)
                    && !out.iter().any(|x| x.id == id)
                {
                    out.push(Member {
                        id,
                        name: name.clone(),
                    });
                }
            }
            out.insert(
                0,
                Member {
                    id: SELF_ID,
                    name: me().name,
                },
            );
        }
        out.truncate(limit);
        Ok(out)
    }

    async fn download_photo(&self, _chat: ChatId, id: MessageId) -> Result<Vec<u8>> {
        self.call("download_photo").await?;
        Ok(mock_photo_png(i64::from(id), 640, 427))
    }

    async fn search_global(&self, query: String, limit: usize) -> Result<Vec<GlobalHit>> {
        self.call("search_global").await?;
        self.require_auth()?;
        let q = query.to_lowercase();
        let s = self.st()?;
        let mut hits: Vec<GlobalHit> = Vec::new();
        for c in &s.chats {
            for m in s.history.get(&c.id).into_iter().flatten() {
                if !m.service && m.text.to_lowercase().contains(&q) {
                    hits.push(GlobalHit {
                        message: m.clone(),
                        chat_title: c.title.clone(),
                        chat_kind: c.kind,
                        photo_id: c.photo_id,
                    });
                }
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.message.date));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn download_avatar(&self, _chat: ChatId, photo_id: i64) -> Result<Vec<u8>> {
        self.call("download_avatar").await?;
        mock_avatar_png(photo_id, 160)
    }

    async fn save_draft(
        &self,
        chat: ChatId,
        text: String,
        reply_to: Option<MessageId>,
    ) -> Result<()> {
        self.call("save_draft").await?;
        self.require_auth()?;
        let draft = (!text.is_empty()).then(|| crate::model::Draft {
            text,
            reply_to,
            date: chrono::Utc::now().timestamp(),
        });
        if let Some(c) = self.st()?.chats.iter_mut().find(|c| c.id == chat) {
            c.draft = draft;
        }
        Ok(())
    }

    async fn set_typing(&self, _chat: ChatId, active: bool) -> Result<()> {
        self.call(if active {
            "set_typing:on"
        } else {
            "set_typing:off"
        })
        .await
    }

    async fn download_media(&self, chat: ChatId, id: MessageId, path: PathBuf) -> Result<()> {
        self.call("download_media").await?;
        let media = self
            .st()?
            .history
            .get(&chat)
            .and_then(|h| h.iter().find(|m| m.id == id))
            .and_then(|m| m.media.clone());
        let bytes = match media {
            Some(Media::Photo { .. }) => mock_photo_png(i64::from(id), 640, 427),
            Some(Media::File { name, .. }) => format!("mock contents of {name}\n").into_bytes(),
            _ => return Err(Error::Protocol("no media".into())),
        };
        tokio::fs::write(&path, bytes).await?;
        Ok(())
    }

    async fn peer_info(&self, chat: ChatId) -> Result<PeerInfo> {
        self.call("peer_info").await?;
        self.require_auth()?;
        let mut info = mock_peer_info(chat);
        info.blocked = self.st()?.blocked.contains(&chat);
        Ok(info)
    }

    async fn set_mute(&self, chat: ChatId, until: i64) -> Result<()> {
        self.call(if until == 0 { "unmute" } else { "mute" })
            .await?;
        self.require_auth()?;
        let settings = {
            let mut s = self.st()?;
            let c = s
                .chats
                .iter_mut()
                .find(|c| c.id == chat)
                .ok_or_else(|| Error::Protocol("unknown peer".into()))?;
            c.notify.mute_until = Some(until);
            c.notify
        };
        self.events.emit(Event::Notify {
            target: NotifyTarget::Chat(chat),
            settings,
        });
        Ok(())
    }

    async fn notify_defaults(&self) -> Result<NotifyDefaults> {
        self.call("notify_defaults").await?;
        self.require_auth()?;
        Ok(self.st()?.notify_defaults)
    }

    async fn clear_history(&self, chat: ChatId, revoke: bool) -> Result<()> {
        self.call(if revoke {
            "clear_history+revoke"
        } else {
            "clear_history"
        })
        .await?;
        self.require_auth()?;
        let mut s = self.st()?;
        s.history.insert(chat, Vec::new());
        if let Some(c) = s.chats.iter_mut().find(|c| c.id == chat) {
            c.last_message = None;
            c.unread_count = 0;
        }
        Ok(())
    }

    async fn delete_chat(&self, chat: ChatId, revoke: bool) -> Result<()> {
        let name = if chat > 0 {
            if revoke {
                "delete_chat+revoke"
            } else {
                "delete_chat"
            }
        } else if chat > -CHANNEL_OFFSET {
            "leave_group"
        } else {
            "leave_channel"
        };
        self.call(name).await?;
        self.require_auth()?;
        let mut s = self.st()?;
        s.chats.retain(|c| c.id != chat);
        s.history.remove(&chat);
        Ok(())
    }

    async fn set_blocked(&self, chat: ChatId, blocked: bool) -> Result<()> {
        self.call(if blocked { "block" } else { "unblock" }).await?;
        self.require_auth()?;
        let mut s = self.st()?;
        if blocked {
            s.blocked.insert(chat);
        } else {
            s.blocked.remove(&chat);
        }
        Ok(())
    }

    async fn shared_media(
        &self,
        chat: ChatId,
        kind: SharedKind,
        offset_id: MessageId,
        limit: usize,
    ) -> Result<SharedPage> {
        self.call("shared_media").await?;
        self.require_auth()?;
        let all = mock_shared(chat, kind);
        let older: Vec<SharedItem> = all
            .into_iter()
            .filter(|i| offset_id <= 0 || i.id < offset_id)
            .collect();
        let items: Vec<SharedItem> = older.iter().take(limit).cloned().collect();
        let next = (older.len() > limit)
            .then(|| items.last().map(|i| i.id))
            .flatten();
        Ok(SharedPage { items, next })
    }

    async fn download_thumb(&self, _chat: ChatId, id: MessageId) -> Result<Vec<u8>> {
        self.call("download_thumb").await?;
        Ok(mock_photo_png(i64::from(id), 160, 120))
    }

    async fn dialog_filters(&self) -> Result<Vec<Folder>> {
        self.call("dialog_filters").await?;
        self.require_auth()?;
        Ok(self.st()?.folders.clone())
    }

    async fn save_dialog_filter(&self, folder: Folder) -> Result<()> {
        self.call("save_dialog_filter").await?;
        self.require_auth()?;
        // The same checks the server makes.
        if folder.title.trim().is_empty() {
            return Err(Error::Protocol("400 FILTER_TITLE_EMPTY".into()));
        }
        if !folder.has_rules() {
            return Err(Error::Protocol("400 FILTER_INCLUDE_EMPTY".into()));
        }
        let mut s = self.st()?;
        match s.folders.iter_mut().find(|f| f.id == folder.id) {
            Some(slot) => *slot = folder.clone(),
            None => {
                if s.folders.len() >= MOCK_MAX_FOLDERS {
                    return Err(Error::Protocol("400 DIALOG_FILTERS_TOO_MUCH".into()));
                }
                s.folders.push(folder.clone());
            }
        }
        drop(s);
        self.events.emit(Event::FolderChanged {
            id: folder.id,
            folder: Some(folder),
        });
        Ok(())
    }

    async fn delete_dialog_filter(&self, id: i32) -> Result<()> {
        self.call("delete_dialog_filter").await?;
        self.require_auth()?;
        self.st()?.folders.retain(|f| f.id != id);
        self.events.emit(Event::FolderChanged { id, folder: None });
        Ok(())
    }

    async fn reorder_dialog_filters(&self, order: Vec<i32>) -> Result<()> {
        self.call("reorder_dialog_filters").await?;
        self.require_auth()?;
        let mut s = self.st()?;
        let mut rest = std::mem::take(&mut s.folders);
        let mut out = Vec::new();
        for id in &order {
            if let Some(i) = rest.iter().position(|f| f.id == *id) {
                out.push(rest.remove(i));
            }
        }
        out.extend(rest);
        s.folders = out;
        drop(s);
        self.events.emit(Event::FoldersOrder(order));
        Ok(())
    }

    async fn dh_config(&self) -> Result<crate::net::calls::dh::DhConfig> {
        self.call("dh_config").await?;
        Ok(crate::mock_call::dh_config())
    }

    async fn request_call(
        &self,
        user: ChatId,
        g_a_hash: Vec<u8>,
    ) -> Result<crate::net::calls::PhoneCall> {
        self.call("request_call").await?;
        self.phone.request_call(user, g_a_hash).await
    }

    async fn received_call(&self, _call: crate::net::calls::CallRef) -> Result<()> {
        self.call("received_call").await
    }

    async fn accept_call(
        &self,
        call: crate::net::calls::CallRef,
        g_b: Vec<u8>,
    ) -> Result<crate::net::calls::PhoneCall> {
        self.call("accept_call").await?;
        self.phone.accept_call(call, g_b).await
    }

    async fn confirm_call(
        &self,
        call: crate::net::calls::CallRef,
        g_a: Vec<u8>,
        key_fingerprint: i64,
    ) -> Result<crate::net::calls::PhoneCall> {
        self.call("confirm_call").await?;
        self.phone.confirm_call(call, g_a, key_fingerprint).await
    }

    async fn discard_call(
        &self,
        call: crate::net::calls::CallRef,
        duration: i32,
        reason: crate::net::calls::DiscardReason,
    ) -> Result<()> {
        self.call("discard_call").await?;
        self.phone.discard(call, duration, reason)
    }

    async fn send_call_signaling(
        &self,
        call: crate::net::calls::CallRef,
        data: Vec<u8>,
    ) -> Result<()> {
        self.phone.signaling(call, data)
    }

    fn events(&self) -> BoxStream<'static, Event> {
        self.events.subscribe()
    }
}

/// Info panel data of the mock account's chats.
pub fn mock_peer_info(chat: ChatId) -> PeerInfo {
    match chat {
        ALICE => PeerInfo {
            about: Some("Hiking, coffee and a little bit of Rust.".into()),
            username: Some("alice_j".into()),
            phone: Some("+34 600 111 222".into()),
            members: None,
            blocked: false,
        },
        RUST_GROUP => PeerInfo {
            about: Some("Rust enthusiasts in Madrid. Be kind, share code, bring snacks.".into()),
            username: None,
            phone: None,
            members: Some(4_721),
            blocked: false,
        },
        DEV_CHANNEL => PeerInfo {
            about: Some("Release notes and news about Ferrogram.".into()),
            username: Some("ferrogram_dev".into()),
            phone: None,
            members: Some(12_840),
            blocked: false,
        },
        c if c > 0 => PeerInfo {
            about: None,
            username: None,
            phone: None,
            members: None,
            blocked: false,
        },
        _ => PeerInfo {
            about: Some("A group chat.".into()),
            username: None,
            phone: None,
            members: Some(12),
            blocked: false,
        },
    }
}

/// How many shared items each tab has in the mock account (Alice's chat is
/// big enough to need two pages of media).
fn mock_shared_count(chat: ChatId, kind: SharedKind) -> usize {
    let big = chat == ALICE;
    let small = chat == RUST_GROUP || chat == DEV_CHANNEL;
    match (kind, big, small) {
        (SharedKind::Media, true, _) => 64,
        (SharedKind::Files, true, _) => 17,
        (SharedKind::Links, true, _) => 14,
        (SharedKind::Music, true, _) => 9,
        (SharedKind::Voice, true, _) => 7,
        (SharedKind::Gifs, true, _) => 11,
        (SharedKind::Media, _, true) => 12,
        (SharedKind::Files, _, true) => 6,
        (SharedKind::Links, _, true) => 9,
        (SharedKind::Gifs, _, true) => 3,
        _ => 0,
    }
}

/// Deterministic shared items of a chat tab, newest first.
pub fn mock_shared(chat: ChatId, kind: SharedKind) -> Vec<SharedItem> {
    const FILES: [(&str, u64, &str); 5] = [
        ("trail-map.pdf", 2_411_000, "application/pdf"),
        ("itinerary.docx", 48_200, "application/msword"),
        ("photos-backup.zip", 18_300_000, "application/zip"),
        ("budget.xlsx", 91_500, "application/vnd.ms-excel"),
        ("notes.txt", 1_240, "text/plain"),
    ];
    const SITES: [(&str, &str, &str); 4] = [
        (
            "https://doc.rust-lang.org/book/",
            "The Rust Programming Language",
            "doc.rust-lang.org",
        ),
        (
            "https://www.komoot.com/tour/123456",
            "Sierra de Guadarrama loop",
            "komoot.com",
        ),
        (
            "https://en.wikipedia.org/wiki/Hiking",
            "Hiking - Wikipedia",
            "wikipedia.org",
        ),
        (
            "https://github.com/FerroApps/Ferrogram",
            "FerroApps/Ferrogram",
            "github.com",
        ),
    ];
    const SONGS: [(&str, &str); 3] = [
        ("Mountain Air", "The Trailheads"),
        ("Coffee at Dawn", "Alba Marin"),
        ("Long Way Home", "Quiet Rivers"),
    ];
    let base = 3_000 - 211 * (kind as i32);
    (0..mock_shared_count(chat, kind))
        .map(|i| {
            let id = base - 13 * i as i32;
            let date = MOCK_NOW - 3_600 * (i as i64 * 7 + 3);
            let body = match kind {
                SharedKind::Media if i % 5 == 3 => SharedBody::Video {
                    width: 1280,
                    height: 720,
                    seconds: 12 + 31 * (i as u32 % 7),
                },
                SharedKind::Media => SharedBody::Photo {
                    width: 800,
                    height: 600,
                },
                SharedKind::Gifs => SharedBody::Gif {
                    width: 480,
                    height: 270,
                },
                SharedKind::Files => {
                    let (n, size, mime) = FILES[i % FILES.len()];
                    SharedBody::File {
                        name: if i < FILES.len() {
                            n.to_owned()
                        } else {
                            format!("{i}-{n}")
                        },
                        size,
                        mime: mime.into(),
                    }
                }
                SharedKind::Links => {
                    let (url, title, site) = SITES[i % SITES.len()];
                    SharedBody::Link {
                        url: url.into(),
                        title: Some(title.into()),
                        description: (i % 2 == 0)
                            .then(|| "Everything you need to get started, in one place.".into()),
                        site: Some(site.into()),
                    }
                }
                SharedKind::Music => {
                    let (t, a) = SONGS[i % SONGS.len()];
                    SharedBody::Audio {
                        title: Some(t.into()),
                        performer: Some(a.into()),
                        name: None,
                        seconds: 150 + 17 * i as u32,
                    }
                }
                SharedKind::Voice => SharedBody::Voice {
                    seconds: 4 + 9 * (i as u32 % 5),
                },
            };
            SharedItem { id, date, body }
        })
        .collect()
}

/// The server's folder limit in mock mode.
pub const MOCK_MAX_FOLDERS: usize = 10;

/// Folders of the mock account (shown in mock mode and screenshots).
pub fn mock_folders() -> Vec<Folder> {
    let mut work = Folder::new(2, "Work");
    work.groups = true;
    work.include = vec![ECHO_BOT];
    work.exclude_muted = true;
    let mut people = Folder::new(3, "People");
    people.contacts = true;
    people.non_contacts = true;
    people.exclude = vec![ECHO_BOT];
    let mut news = Folder::new(4, "News");
    news.channels = true;
    news.exclude_read = true;
    vec![work, people, news]
}

/// A procedurally generated landscape-ish PNG (sky gradient, mountains),
/// so mock mode has a "photo" without shipping any image assets.
pub fn mock_photo_png(seed: i64, w: u32, h: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let s = (seed.rem_euclid(97)) as f32 / 97.0;
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let ridge1 =
                0.55 + 0.12 * ((fx * 9.0 + s * 3.0).sin() * 0.6 + (fx * 23.0).sin() * 0.25);
            let ridge2 = 0.72 + 0.08 * ((fx * 5.0 + 1.3).sin() + (fx * 17.0 + s).sin() * 0.3);
            let (r, g, b) = if fy > ridge2 {
                (40.0 + 30.0 * fy, 70.0 + 40.0 * fy, 50.0)
            } else if fy > ridge1 {
                (70.0 + 20.0 * fx, 85.0 + 25.0 * fx, 120.0 + 20.0 * fy)
            } else {
                let t = fy / ridge1;
                (
                    250.0 - 120.0 * t + 20.0 * s,
                    170.0 - 40.0 * t,
                    120.0 + 100.0 * t,
                )
            };
            rgba.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
        }
    }
    encode_png(w, h, &rgba).unwrap_or_default()
}

/// A flat-coloured avatar with a lighter diagonal band, standing in for a
/// downloaded profile photo (no image assets are shipped).
pub fn mock_avatar_png(seed: i64, size: u32) -> Result<Vec<u8>> {
    const PALETTE: [(u8, u8, u8); 5] = [
        (0xE1, 0x70, 0x76),
        (0x65, 0xAA, 0xDD),
        (0xA6, 0x95, 0xE7),
        (0x7B, 0xC8, 0x62),
        (0xEE, 0xA2, 0x74),
    ];
    let (r, g, b) = PALETTE[seed.rem_euclid(5) as usize];
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let band = (x + y) % 64 < 16;
            let k = if band { 1.25 } else { 1.0 };
            let f = |c: u8| (f32::from(c) * k).min(255.0) as u8;
            rgba.extend_from_slice(&[f(r), f(g), f(b), 255]);
        }
    }
    encode_png(size, size, &rgba)
}

/// Encode RGBA8 as PNG.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc
            .write_header()
            .map_err(|_| Error::Io("png header".into()))?;
        writer
            .write_image_data(rgba)
            .map_err(|_| Error::Io("png data".into()))?;
    }
    Ok(out)
}

/// A release one minor version ahead of this build (update screenshots and
/// tests; never fetched).
pub fn mock_release() -> crate::update::check::Release {
    let c = crate::update::Version::current();
    let v = crate::update::Version::new(c.major, c.minor + 1, 0);
    let base = format!("https://github.com/{}/releases", crate::update::REPO);
    let name = format!("ferrogram-v{v}-linux-x86_64.tar.gz");
    crate::update::check::Release {
        version: v,
        tag: format!("v{v}"),
        page: format!("{base}/tag/v{v}"),
        assets: vec![
            crate::update::check::Asset {
                url: format!("{base}/download/v{v}/{name}"),
                name: name.clone(),
                size: 30 * 1024 * 1024,
            },
            crate::update::check::Asset {
                url: format!("{base}/download/v{v}/{name}.sha256"),
                name: format!("{name}.sha256"),
                size: 100,
            },
        ],
    }
}
