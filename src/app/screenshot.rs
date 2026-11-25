//! `--mock --screenshot <screen> <out.png>`: drive the app through a short
//! script with mock data, capture the window and exit (architecture §10).

use std::time::Duration;

use iced::{Task, window};

use super::{Done, Message, Ui};
use crate::config::{Config, ScreenshotTarget};
use crate::mock::{ALICE, CODE_2FA, RUST_GROUP};
use crate::model::TypingAction;
use crate::net::Event;

fn after(ms: u64, msg: Message) -> Task<Message> {
    Task::perform(
        async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        },
        move |_| msg.clone(),
    )
}

fn ui(u: Ui) -> Message {
    Message::Ui(u)
}

/// A check result announcing the mock release.
fn update_found() -> Message {
    Message::Done(Done::UpdateChecked {
        manual: false,
        result: Ok(Some(crate::mock::mock_release())),
    })
}

/// Timed steps for each screen; the last one captures the window.
pub fn script(config: &Config) -> Task<Message> {
    let Some((target, _)) = config.screenshot else {
        return Task::none();
    };
    let steps: Vec<(u64, Message)> = match target {
        ScreenshotTarget::ApiSetup => vec![
            (300, ui(Ui::OpenApiSetup)),
            (350, ui(Ui::SetupApiId("123456".into()))),
        ],
        ScreenshotTarget::LoginPhone => vec![(300, ui(Ui::AuthInput("+34 600 123 412".into())))],
        ScreenshotTarget::LoginCode => vec![
            (300, ui(Ui::AuthInput("+34 600 123 412".into()))),
            (350, ui(Ui::AuthSubmit)),
            (700, ui(Ui::AuthInput("123".into()))),
        ],
        ScreenshotTarget::LoginPassword => vec![
            (300, ui(Ui::AuthInput("+34 600 123 412".into()))),
            (350, ui(Ui::AuthSubmit)),
            (700, ui(Ui::AuthInput(CODE_2FA.into()))),
            (750, ui(Ui::AuthSubmit)),
            (1100, ui(Ui::AuthInput("password".into()))),
        ],
        ScreenshotTarget::Chats => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (
                600,
                Message::Net(Event::Typing {
                    chat: RUST_GROUP,
                    user: 311,
                    action: Some(TypingAction::Typing),
                }),
            ),
            (900, ui(Ui::ComposerChanged(String::new()))),
        ],
        ScreenshotTarget::Search => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::SidebarSearch("hik".into()))),
            (1000, ui(Ui::ToggleChatSearch)),
            (1100, ui(Ui::ChatSearchInput("trail".into()))),
            (1150, ui(Ui::ChatSearchSubmit)),
        ],
        ScreenshotTarget::Reply => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::Reply(148))),
            (
                950,
                ui(Ui::ComposerChanged(
                    "Coffee sounds amazing, see you there".into(),
                )),
            ),
            (1000, ui(Ui::MessageMenu(Some(145)))),
        ],
        ScreenshotTarget::Emoji => vec![
            (400, ui(Ui::SelectChat(crate::mock::EMOJI_GROUP))),
            (
                900,
                ui(Ui::ComposerChanged(
                    "On my way \u{1F680} 1\u{FE0F}\u{20E3} \u{1F1EA}\u{1F1F8} \u{1F44D}\u{1F3FD} \u{A9}\u{FE0F}".into(),
                )),
            ),
        ],
        ScreenshotTarget::Settings => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::OpenSettings)),
        ],
        ScreenshotTarget::Folders => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::SelectFolder(Some(2)))),
        ],
        ScreenshotTarget::FolderManager => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::OpenFolders)),
            (950, ui(Ui::FolderDeleteAsk(4))),
        ],
        ScreenshotTarget::FolderEditor => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::FolderEdit(2))),
        ],
        ScreenshotTarget::FolderPicker => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::FolderEdit(2))),
            (950, ui(Ui::FolderPick(Some(crate::app::folders::Picker::Include)))),
        ],
        ScreenshotTarget::ChatMenu => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::ChatMenu(Some(RUST_GROUP)))),
        ],
        ScreenshotTarget::HeaderMenu => vec![
            (400, ui(Ui::SelectChat(RUST_GROUP))),
            (900, ui(Ui::ToggleHeaderMenu)),
        ],
        ScreenshotTarget::HeaderMute => vec![
            (400, ui(Ui::SelectChat(RUST_GROUP))),
            (900, ui(Ui::ToggleHeaderMenu)),
            (950, ui(Ui::HeaderMenuPage(crate::app::chatmenu::HeaderMenu::Mute))),
        ],
        ScreenshotTarget::HeaderConfirm => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::AskClearHistory)),
        ],
        ScreenshotTarget::Select => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, ui(Ui::StartSelect)),
            (950, ui(Ui::ToggleSelect(147))),
            (960, ui(Ui::ToggleSelect(148))),
        ],
        ScreenshotTarget::SearchFrom => vec![
            (400, ui(Ui::SelectChat(RUST_GROUP))),
            (900, ui(Ui::OpenChatSearch)),
            (950, ui(Ui::ChatSearchFromPicker)),
        ],
        ScreenshotTarget::Info(kind) => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (700, ui(Ui::ToggleInfo)),
            (1000, ui(Ui::InfoTab(kind))),
            (3000, ui(Ui::Tick)),
        ],
        ScreenshotTarget::Media => vec![
            (400, ui(Ui::SelectChat(RUST_GROUP))),
            (500, ui(Ui::SelectChat(ALICE))),
            (1200, ui(Ui::OpenMedia(ALICE, 143))),
        ],
        ScreenshotTarget::CallIncoming => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (600, Message::Net(Event::PhoneCall(calls::incoming()))),
        ],
        ScreenshotTarget::CallOutgoing => calls::outgoing(),
        ScreenshotTarget::CallActive => calls::active(),
        ScreenshotTarget::UpdateBanner => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, update_found()),
        ],
        ScreenshotTarget::UpdateSettings => vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (900, update_found()),
            (950, ui(Ui::OpenSettings)),
        ],
        ScreenshotTarget::CallBar => {
            let mut s = calls::active();
            s.push((1100, ui(Ui::CallExpand(false))));
            s
        }
    };
    let last = steps.iter().map(|s| s.0).max().unwrap_or(0);
    let mut tasks: Vec<Task<Message>> = steps.into_iter().map(|(t, m)| after(t, m)).collect();
    tasks.push(
        Task::perform(
            async move { tokio::time::sleep(Duration::from_millis(last + 1500)).await },
            |_| (),
        )
        .then(|_| window::latest())
        .and_then(window::screenshot)
        .map(|s| Message::Done(Done::Screenshot(s))),
    );
    Task::batch(tasks)
}

pub fn save(config: &Config, shot: window::Screenshot) -> Task<Message> {
    let Some((_, path)) = config.screenshot.clone() else {
        return Task::none();
    };
    let w = shot.size.width;
    let h = shot.size.height;
    let rgba = shot.rgba.to_vec();
    Task::perform(
        async move {
            match crate::mock::encode_png(w, h, &rgba) {
                Ok(png) => {
                    if let Err(e) = std::fs::write(&path, png) {
                        eprintln!("screenshot write failed: {e}");
                    } else {
                        eprintln!("screenshot saved: {} ({w}x{h})", path.display());
                    }
                }
                Err(e) => eprintln!("screenshot encode failed: {e}"),
            }
        },
        |_| Message::Done(Done::ScreenshotSaved),
    )
}

/// Call screens: the states are injected (as the server and engine would
/// report them) so the images do not depend on timing.
mod calls {
    use super::{ALICE, Done, Event, Message, Ui, ui};
    use crate::net::calls::dh::{self, CallKey};
    use crate::net::calls::engine::{EngineEvent, LinkState};
    use crate::net::calls::{CallRef, PhoneCall, Protocol};

    const CALL: CallRef = CallRef {
        id: 4242,
        access_hash: 1,
    };

    pub fn incoming() -> PhoneCall {
        PhoneCall::Requested {
            call: CALL,
            admin_id: ALICE,
            participant_id: crate::mock::me().id,
            video: false,
            g_a_hash: vec![0; 32],
            protocol: Protocol::ours(),
        }
    }

    pub fn outgoing() -> Vec<(u64, Message)> {
        vec![
            (400, ui(Ui::SelectChat(ALICE))),
            (600, ui(Ui::CallStart(ALICE))),
            (
                700,
                Message::Net(Event::PhoneCall(PhoneCall::Waiting {
                    call: CALL,
                    admin_id: crate::mock::me().id,
                    participant_id: ALICE,
                    video: false,
                    receive_date: Some(1),
                })),
            ),
        ]
    }

    pub fn active() -> Vec<(u64, Message)> {
        let key = CallKey([7; 256]);
        let emoji = dh::emoji_fingerprint(&key, &[1; 256]);
        let mut s = outgoing();
        s.push((
            800,
            Message::Done(Done::CallKey(Ok(Box::new((key, emoji))))),
        ));
        s.push((
            900,
            ui(Ui::CallEngine(EngineEvent::State(LinkState::Connected {
                relay: false,
            }))),
        ));
        s.push((950, ui(Ui::CallEngine(EngineEvent::Quality(4)))));
        s
    }
}
