//! The `Message` enum: UI events, network events and task completions.

use std::path::PathBuf;

use iced::widget::scrollable::Viewport;

use crate::app::chatmenu::{Dir, HeaderMenu};
use crate::app::folders::{EditorFlag, Picker};
use crate::auth::{SecretInput, SignInOutcome};
use crate::error::Error;
use crate::model::{
    Chat, ChatId, Folder, GlobalHit, Member, Message as ChatMessage, MessageId, MuteFor,
    NotifyDefaults, NotifySettings, PeerInfo, SharedKind, SharedPage, User,
};
use crate::net::{ApiHandle, Event};

#[derive(Debug, Clone)]
pub enum Message {
    Ui(Ui),
    Net(Event),
    Done(Done),
}

#[derive(Debug, Clone)]
pub enum Ui {
    // first-run API credential setup
    SetupApiId(String),
    SetupApiHash(SecretInput),
    SetupSubmit,
    /// "Advanced: use your own API credentials" on the login screen.
    OpenApiSetup,
    CancelSetup,
    /// Remove the user override and use the built-in credentials.
    UseBuiltinCredentials,
    // login
    AuthInput(SecretInput),
    AuthSubmit,
    AuthBack,
    // sidebar
    SelectChat(ChatId),
    SidebarSearch(String),
    /// The chat list scrolled: load the next page near its end.
    SidebarScrolled(Viewport),
    ClearSidebarSearch,
    /// Open a chat at a specific message from global/in-chat search.
    OpenResult(ChatId, MessageId),
    /// The sidebar query settled (debounce): run the server-side search.
    GlobalSearchDue(String),
    /// The composer was idle for a second: sync the draft of that chat
    /// (only if `version` is still current).
    DraftDue(ChatId, u64),
    // chat pane
    HistoryScrolled(Viewport),
    ComposerChanged(String),
    Send,
    Attach,
    FilePicked(Option<PathBuf>),
    FileDropped(PathBuf),
    SendPendingFile,
    /// Ctrl+V: look for copied files or an image on the system clipboard
    /// (text is pasted by the focused text field itself).
    Paste,
    /// The composer inserted this pasted text (to undo it when the clipboard
    /// turns out to hold a file whose path was offered as text too).
    TextPasted(String),
    /// "Copy image" on a photo message.
    CopyImage(ChatId, MessageId),
    // chat info panel
    /// Header click: open or close the info panel of the open chat.
    ToggleInfo,
    InfoTab(SharedKind),
    /// The panel scrolled: load the next page of the tab near its end.
    InfoScrolled(Viewport),
    /// Notifications switch (`true` = not muted).
    SetNotifications(bool),
    /// Open a shared link in the browser.
    OpenLink(String),
    // chat folders
    /// A folder tab (`None` = All chats).
    SelectFolder(Option<i32>),
    /// Right click on a folder tab: toggle its small menu.
    FolderTabMenu(Option<i32>),
    /// Right click on a chat row: toggle its menu ("Add to folder").
    ChatMenu(Option<ChatId>),
    /// Add the chat to / remove it from a folder.
    ChatInFolder(ChatId, i32),
    OpenFolders,
    CloseFolders,
    /// Move a folder up (-1) or down (+1) in the order.
    FolderMove(i32, isize),
    FolderDeleteAsk(i32),
    FolderDeleteConfirm(i32),
    FolderDeleteCancel,
    /// Open the editor for a new folder (optionally with one chat in it).
    FolderNew(Option<ChatId>),
    FolderEdit(i32),
    FolderTitle(String),
    FolderFlag(EditorFlag),
    /// Open (`Some`) or close (`None`) the chat picker of the editor.
    FolderPick(Option<Picker>),
    FolderPickQuery(String),
    FolderChatToggle(Picker, ChatId),
    FolderChatRemove(Picker, ChatId),
    FolderSave,
    FolderEditorClose,
    CancelPendingFile,
    ToggleEmoji,
    InsertEmoji(&'static str),
    MessageMenu(Option<MessageId>),
    Reply(MessageId),
    Edit(MessageId),
    Delete(MessageId),
    Copy(MessageId),
    React(MessageId, String),
    SaveMedia(ChatId, MessageId),
    CancelCompose,
    ToggleChatSearch,
    /// Open (or keep open) the in-chat search and focus its field.
    OpenChatSearch,
    ChatSearchInput(String),
    /// Enter in the search field: search now, or go to the next hit.
    ChatSearchSubmit,
    /// The query settled (debounce); only the latest version searches.
    ChatSearchDue(u64),
    ChatSearchStep(Dir),
    /// Pick (`Some`) or clear (`None`) the "from:" member.
    ChatSearchFrom(Option<Member>),
    /// Show or hide the member picker.
    ChatSearchFromPicker,
    ChatSearchFromQuery(String),
    // chat header menu
    ToggleHeaderMenu,
    HeaderMenuPage(HeaderMenu),
    CloseHeaderMenu,
    /// Mute the open chat (or `MuteFor::Off` to unmute).
    SetMute(MuteFor),
    AskClearHistory,
    AskDeleteChat,
    AskLeave,
    /// The "also delete for the other side" checkbox of the open dialog.
    ConfirmRevoke(bool),
    ConfirmAccept,
    ConfirmCancel,
    ToggleBlock,
    // message selection
    StartSelect,
    ToggleSelect(MessageId),
    CancelSelect,
    AskDeleteSelected,
    CopySelected,
    ScrollToBottom,
    OpenMedia(ChatId, MessageId),
    CloseMedia,
    LogOut,
    ToggleMainMenu,
    // background mode
    /// A request from the tray, a second launch or a notification click.
    Remote(crate::remote::Remote),
    // one-to-one calls
    /// The phone button in the chat header or the info panel.
    CallStart(ChatId),
    CallAccept,
    /// Hang up, cancel or decline.
    CallHangUp,
    CallToggleMute,
    /// Show the full call panel (`true`) or just the bar above the chat.
    CallExpand(bool),
    /// Open or close the microphone / speaker list.
    CallDevices(Option<crate::app::phone::DeviceKind>),
    CallPickDevice(crate::app::phone::DeviceKind, String),
    CallTick,
    CallEngine(crate::net::calls::engine::EngineEvent),
    /// The window's close button (or Alt+F4): to the tray, or quit.
    CloseRequested,
    WindowResized(iced::Size),
    WindowMoved(iced::Point),
    /// The window's scale factor (on open and when it changes): shared-media
    /// thumbnails are made at the size they are drawn.
    Rescaled(f32),
    OpenSettings,
    CloseSettings,
    SetCloseToTray(bool),
    SetStartMinimized(bool),
    SetAutostart(bool),
    DismissToast,
    // updates (src/update, app/updates.rs)
    /// A timed check is due (only acts for the current schedule generation).
    UpdateCheckDue(u64),
    /// "Check for updates" in Settings.
    UpdateCheckNow,
    /// [Update] / "Update now": download and install (install.sh only).
    UpdateInstall,
    UpdateCancel,
    /// [x] on the banner, or "Later" after an install.
    UpdateDismiss,
    UpdateRestart,
    /// "Release notes" / "Open release page".
    UpdateReleaseNotes,
    /// Copy the `cargo install` command.
    UpdateCopyCommand,
    SetCheckUpdates(bool),
    // window / keyboard
    Focus(bool),
    Escape,
    Tick,
    /// 1 s while somebody is typing: expire stale "typing…" statuses.
    TypingTick,
    /// Click on a received file: open with the system handler (or save,
    /// for executables).
    OpenFile(ChatId, MessageId),
}

#[derive(Debug, Clone)]
pub enum Done {
    ApiReady(Result<ApiHandle, Error>),
    Restored(Result<Option<User>, Error>),
    /// Retry `restore()` after a network failure (backoff elapsed).
    RetryRestore,
    CredentialsSaved(Result<(), Error>),
    /// The old client was shut down; start a new one with fresh credentials.
    RestartApi,
    CodeRequested(Result<(), Error>),
    SignIn(Result<SignInOutcome, Error>),
    Persisted(Result<(), Error>),
    Dialogs(Result<Vec<Chat>, Error>),
    /// The next page of dialogs (appended to the list).
    MoreDialogs(Result<Vec<Chat>, Error>),
    CacheHistory {
        chat: ChatId,
        before: Option<MessageId>,
        /// The cache page is inside the network-confirmed range.
        complete: bool,
        result: Result<Vec<ChatMessage>, Error>,
    },
    NetHistory {
        chat: ChatId,
        before: Option<MessageId>,
        result: Result<Vec<ChatMessage>, Error>,
    },
    Sent {
        chat: ChatId,
        temp: MessageId,
        result: Result<ChatMessage, Error>,
    },
    /// An optimistic edit/delete failed: restore `old`.
    Rollback {
        old: ChatMessage,
        error: Error,
    },
    /// A fire-and-forget call (react, mark read, cache write) finished.
    Call(Result<(), Error>),
    /// One page of the in-chat search for query `version`.
    SearchPage {
        chat: ChatId,
        version: u64,
        offset: MessageId,
        result: Result<Vec<ChatMessage>, Error>,
    },
    /// Members for the search "from:" picker.
    Members {
        chat: ChatId,
        result: Result<Vec<Member>, Error>,
    },
    /// The account-wide notification defaults.
    NotifyDefaults(Result<NotifyDefaults, Error>),
    /// A clear-history / delete / leave call finished.
    ChatActionDone {
        action: crate::app::chatmenu::Confirm,
        result: Result<(), Error>,
    },
    BlockDone {
        chat: ChatId,
        blocked: bool,
        result: Result<(), Error>,
    },
    /// Several optimistic deletes failed: restore `old`.
    RollbackMany {
        old: Vec<ChatMessage>,
        error: Error,
    },
    GlobalResults {
        query: String,
        result: Result<Vec<GlobalHit>, Error>,
    },
    SidebarResults {
        query: String,
        result: Result<Vec<ChatMessage>, Error>,
    },
    FileStat {
        path: PathBuf,
        result: Result<(u64, bool), Error>,
    },
    SavePicked {
        chat: ChatId,
        id: MessageId,
        path: Option<PathBuf>,
    },
    MediaSaved(Result<(), Error>),
    PeerInfo {
        chat: ChatId,
        result: Result<PeerInfo, Error>,
    },
    SharedPage {
        chat: ChatId,
        kind: SharedKind,
        offset: MessageId,
        result: Result<SharedPage, Error>,
    },
    /// A shared-media preview was downloaded (encoded bytes).
    Thumb {
        key: (ChatId, MessageId),
        result: Result<Vec<u8>, Error>,
    },
    /// The same preview, decoded and cropped off the UI thread.
    ThumbReady {
        key: (ChatId, MessageId),
        result: Result<crate::media::ThumbImage, Error>,
    },
    /// The mute call finished; on failure `previous` is restored.
    MuteDone {
        chat: ChatId,
        previous: NotifySettings,
        result: Result<(), Error>,
    },
    LinkOpened(Result<(), Error>),
    /// The folder list from the server.
    Folders(Result<Vec<Folder>, Error>),
    /// The editor's folder was saved (or refused).
    FolderSaved {
        folder: Folder,
        result: Result<(), Error>,
    },
    /// An optimistic folder change finished; on failure the list goes back
    /// to `before`.
    FolderOp {
        before: Vec<Folder>,
        result: Result<(), Error>,
    },
    /// The clipboard content found after Ctrl+V.
    Pasted(crate::clipboard::Clip),
    /// A pasted image was written to a private temporary file.
    PasteSaved(Result<PathBuf, Error>),
    ImageCopied(Result<(), Error>),
    Photo {
        key: (ChatId, MessageId),
        result: Result<Vec<u8>, Error>,
    },
    /// A profile photo arrived (or failed). `Err(Cancelled)` = not available
    /// right now (offline), retried later.
    Avatar {
        chat: ChatId,
        photo_id: i64,
        result: Result<crate::media::AvatarImage, Error>,
    },
    /// A received file was downloaded and is ready for the system handler.
    FileReady(Result<PathBuf, Error>),
    FileOpened(Result<(), Error>),
    LoggedOut,
    /// The tray registration finished (the text is the failure reason).
    TrayReady(Result<crate::tray::TrayHandle, String>),
    WindowOpened(iced::window::Id),
    /// Autostart was switched; carries the state now on disk.
    AutostartSet(Result<bool, Error>),
    Screenshot(iced::window::Screenshot),
    ScreenshotSaved,
    // updates
    UpdateMethod(crate::update::InstallMethod),
    UpdateChecked {
        manual: bool,
        result: Result<Option<crate::update::check::Release>, crate::update::UpdateError>,
    },
    UpdateProgress(crate::update::install::Progress),
    UpdateInstalled(Result<crate::update::Version, crate::update::UpdateError>),
    // one-to-one calls
    CallConfig(Result<crate::net::calls::CallConfig, Error>),
    CallPlaced(Result<Box<(crate::app::call::DhHalf, crate::net::calls::PhoneCall)>, Error>),
    CallAccepted(Result<Box<(crate::app::call::DhHalf, crate::net::calls::PhoneCall)>, Error>),
    CallConfirmed(Result<Box<crate::app::call::Confirmed>, Error>),
    CallKey(Result<Box<(crate::net::calls::dh::CallKey, [&'static str; 4])>, Error>),
    CallMedia(crate::app::phone::MediaSlot),
    CallDevicesListed(Vec<String>, Vec<String>),
    /// A `phone.*` request without a result we need finished.
    CallSent(Result<(), Error>),
}
