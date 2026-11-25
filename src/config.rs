//! Paths (XDG via `directories`), feature flags and env overrides.

use std::path::{Path, PathBuf};

use secrecy::SecretString;

use crate::auth::secrets::{Secret, SecretStore};
use crate::error::{AuthError, Error, Result};

pub const APP_ID: &str = "dev.ferro.ferrogram";
pub const APP_NAME: &str = "Ferrogram";

/// Which screen `--screenshot` should render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotTarget {
    LoginPhone,
    LoginCode,
    LoginPassword,
    Chats,
    Search,
    Reply,
    Emoji,
    Media,
    ApiSetup,
    Settings,
    /// Folder tabs with a folder selected.
    Folders,
    /// The "Chat folders" manager dialog.
    FolderManager,
    /// The folder editor (name, chats, types, exclusions).
    FolderEditor,
    /// The chat picker of the folder editor.
    FolderPicker,
    /// A chat row's context menu with "Add to folder".
    ChatMenu,
    HeaderMenu,
    HeaderMute,
    HeaderConfirm,
    Select,
    SearchFrom,
    /// The info panel (profile) with one shared-content tab selected.
    Info(crate::model::SharedKind),
    /// An incoming call ringing.
    CallIncoming,
    /// An outgoing call ringing at the peer.
    CallOutgoing,
    /// A connected call (panel).
    CallActive,
    /// A connected call, minimised to the bar above the chat.
    CallBar,
    /// The "new version available" bar.
    UpdateBanner,
    /// Settings with the Updates section offering an update.
    UpdateSettings,
}

impl ScreenshotTarget {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "login-phone" => Self::LoginPhone,
            "login-code" => Self::LoginCode,
            "login-password" => Self::LoginPassword,
            "chats" => Self::Chats,
            "search" => Self::Search,
            "reply" => Self::Reply,
            "emoji" => Self::Emoji,
            "media" => Self::Media,
            "api-setup" => Self::ApiSetup,
            "settings" => Self::Settings,
            "folders" => Self::Folders,
            "folder-manager" => Self::FolderManager,
            "folder-editor" => Self::FolderEditor,
            "folder-picker" => Self::FolderPicker,
            "chat-menu" => Self::ChatMenu,
            "header-menu" => Self::HeaderMenu,
            "header-mute" => Self::HeaderMute,
            "header-confirm" => Self::HeaderConfirm,
            "select" => Self::Select,
            "search-from" => Self::SearchFrom,
            "info" | "info-media" => Self::Info(crate::model::SharedKind::Media),
            "info-files" => Self::Info(crate::model::SharedKind::Files),
            "info-links" => Self::Info(crate::model::SharedKind::Links),
            "info-music" => Self::Info(crate::model::SharedKind::Music),
            "info-voice" => Self::Info(crate::model::SharedKind::Voice),
            "info-gifs" => Self::Info(crate::model::SharedKind::Gifs),
            "call-incoming" => Self::CallIncoming,
            "call-outgoing" => Self::CallOutgoing,
            "call-active" => Self::CallActive,
            "call-bar" => Self::CallBar,
            "update-banner" => Self::UpdateBanner,
            "update-settings" => Self::UpdateSettings,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    /// Re-creatable files: avatars and files opened with the system handler.
    pub cache_dir: PathBuf,
    pub mock: bool,
    pub screenshot: Option<(ScreenshotTarget, PathBuf)>,
    pub light_theme: bool,
    pub trace_startup: bool,
    /// Notification privacy: when false only the sender is shown.
    pub notification_preview: bool,
    /// Desktop notifications on/off (`FERROGRAM_NO_NOTIFY=1` disables).
    pub notifications: bool,
    /// `--api-setup`: open "use your own API credentials" on start.
    pub api_setup: bool,
    /// `--minimized`: start with no window, only the tray icon.
    pub minimized: bool,
}

impl Config {
    /// Build the config from CLI args and env vars.
    pub fn from_env(args: &[String]) -> Self {
        let dirs = directories::ProjectDirs::from("dev", "ferro", "ferrogram");
        let fallback = std::env::temp_dir().join("ferrogram");
        let data_dir = dirs
            .as_ref()
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| fallback.clone());
        let state_dir = dirs
            .as_ref()
            .and_then(|d| d.state_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| data_dir.join("state"));
        let config_dir = dirs
            .as_ref()
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| fallback.clone());
        let cache_dir = dirs
            .as_ref()
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(|| fallback.join("cache"));

        let mock = args.iter().any(|a| a == "--mock");
        let screenshot = args.iter().position(|a| a == "--screenshot").and_then(|i| {
            let target = ScreenshotTarget::parse(args.get(i + 1)?)?;
            let out = PathBuf::from(args.get(i + 2)?);
            Some((target, out))
        });
        let env_flag = |k: &str| std::env::var(k).is_ok_and(|v| v == "1" || v == "true");
        Self {
            data_dir,
            state_dir,
            config_dir,
            cache_dir,
            mock: mock || screenshot.is_some(),
            screenshot: screenshot.clone(),
            light_theme: std::env::var("FERRO_THEME").is_ok_and(|v| v == "light"),
            trace_startup: env_flag("FERRO_TRACE_STARTUP"),
            notification_preview: !env_flag("FERROGRAM_NOTIFY_SENDER_ONLY"),
            notifications: !env_flag("FERROGRAM_NO_NOTIFY") && screenshot.is_none(),
            api_setup: args.iter().any(|a| a == "--api-setup"),
            minimized: args.iter().any(|a| a == "--minimized"),
        }
    }

    pub fn avatar_dir(&self) -> PathBuf {
        self.cache_dir.join("avatars")
    }

    /// Where files are placed before the system handler opens them.
    pub fn open_dir(&self) -> PathBuf {
        self.cache_dir.join("open")
    }

    /// Images pasted from the clipboard wait here until sent or cancelled
    /// (private files; leftovers are removed at startup).
    pub fn paste_dir(&self) -> PathBuf {
        self.cache_dir.join("paste")
    }

    pub fn cache_db_path(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite3")
    }

    pub fn session_blob_path(&self) -> PathBuf {
        self.data_dir.join("session.enc")
    }

    /// Background-mode preferences (see `prefs`).
    pub fn prefs_path(&self) -> PathBuf {
        self.config_dir.join("settings.ini")
    }

    pub fn log_path(&self) -> PathBuf {
        self.state_dir.join("log")
    }

    fn config_ini(&self) -> PathBuf {
        self.config_dir.join("config.ini")
    }

    /// Telegram API credentials (blocking: touches the keyring, call from a
    /// blocking thread). See [`ApiCredentials::resolve`] for the order.
    pub fn load_api_credentials(&self, secrets: &dyn SecretStore) -> Result<ApiCredentials> {
        let env = |k: &str| std::env::var(k).ok();
        ApiCredentials::resolve(self.gather_credential_inputs(
            secrets,
            &env,
            embedded_credentials(),
        )?)
    }

    /// Collect every credential source (injectable env lookup and embedded
    /// values, so tests can cover each path). Also migrates a legacy
    /// `api_hash` line out of `config.ini` into the keyring.
    pub fn gather_credential_inputs(
        &self,
        secrets: &dyn SecretStore,
        env: &dyn Fn(&str) -> Option<String>,
        embedded: Option<(i32, &'static str)>,
    ) -> Result<CredentialInputs> {
        let path = self.config_ini();
        let mut file = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(legacy) = ini_value(&file, "api_hash")
            && secrets
                .set(API_HASH_KEY, &Secret::new(legacy.into_bytes()))
                .is_ok()
        {
            file = file
                .lines()
                .filter(|l| {
                    l.split_once('=')
                        .is_none_or(|(k, _)| k.trim() != "api_hash")
                })
                .map(|l| format!("{l}\n"))
                .collect();
            let _ = write_private(&path, &file);
            tracing::info!("migrated api_hash from config.ini into the keyring");
        }
        let override_hash = secrets
            .get(API_HASH_KEY)?
            .and_then(|s| String::from_utf8(s.expose().to_vec()).ok());
        Ok(CredentialInputs {
            override_id: ini_value(&file, "api_id"),
            override_hash,
            env_id: env("FERROGRAM_API_ID"),
            env_hash: env("FERROGRAM_API_HASH"),
            embedded,
        })
    }

    /// "Use your own API credentials": `api_id` to `config.ini` (0600),
    /// `api_hash` to the keyring. Blocking.
    pub fn save_api_credentials(
        &self,
        secrets: &dyn SecretStore,
        api_id: i32,
        api_hash: &str,
    ) -> Result<()> {
        secrets.set(
            API_HASH_KEY,
            &Secret::new(api_hash.trim().as_bytes().to_vec()),
        )?;
        ensure_private_dir(&self.config_dir)?;
        write_private(&self.config_ini(), &format!("api_id = {api_id}\n"))?;
        Ok(())
    }

    /// Drop the user override and go back to the built-in credentials.
    pub fn clear_api_override(&self, secrets: &dyn SecretStore) -> Result<()> {
        secrets.delete(API_HASH_KEY)?;
        match std::fs::remove_file(self.config_ini()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Credentials embedded at build time by `build.rs` (validated there).
pub fn embedded_credentials() -> Option<(i32, &'static str)> {
    let id = option_env!("FERROGRAM_EMBEDDED_API_ID")?.parse().ok()?;
    let hash = option_env!("FERROGRAM_EMBEDDED_API_HASH")?;
    Some((id, hash))
}

/// Where the credentials in use came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    /// "Use your own API credentials" (config.ini api_id + keyring api_hash).
    UserOverride,
    /// `FERROGRAM_API_ID` / `FERROGRAM_API_HASH` at runtime (developers).
    Environment,
    /// Ferrogram's own app, baked into release builds.
    Embedded,
}

/// Raw inputs for [`ApiCredentials::resolve`].
#[derive(Default)]
pub struct CredentialInputs {
    pub override_id: Option<String>,
    pub override_hash: Option<String>,
    pub env_id: Option<String>,
    pub env_hash: Option<String>,
    pub embedded: Option<(i32, &'static str)>,
}

impl std::fmt::Debug for CredentialInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialInputs")
            .field(
                "override",
                &(self.override_id.is_some() && self.override_hash.is_some()),
            )
            .field("env", &(self.env_id.is_some() && self.env_hash.is_some()))
            .field("embedded", &self.embedded.is_some())
            .finish()
    }
}

/// `api_hash` must be 32 hex characters.
pub fn valid_api_hash(h: &str) -> bool {
    h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_pair(id: Option<&String>, hash: Option<&String>) -> Option<(i32, String)> {
    let id = id?.trim().parse::<i32>().ok().filter(|n| *n > 0)?;
    let hash = hash?.trim();
    valid_api_hash(hash).then(|| (id, hash.to_owned()))
}

/// Keyring account holding the Telegram `api_hash`.
pub const API_HASH_KEY: &str = "api-hash";

fn ini_value(file: &str, key: &str) -> Option<String> {
    file.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        let v = v.trim().trim_matches('"');
        (k.trim() == key && !v.is_empty()).then(|| v.to_owned())
    })
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = create_private_file(path)?;
    f.write_all(contents.as_bytes())
}

pub struct ApiCredentials {
    pub api_id: i32,
    pub api_hash: SecretString,
    pub source: CredentialSource,
}

impl ApiCredentials {
    /// Resolution order (first complete, well-formed pair wins):
    /// 1. user override ("Advanced: use your own API credentials"),
    /// 2. runtime env vars (developers),
    /// 3. the values embedded in the release build.
    ///
    /// Malformed pairs are skipped (with a warning, values never logged).
    pub fn resolve(i: CredentialInputs) -> Result<Self> {
        let candidates = [
            (
                CredentialSource::UserOverride,
                parse_pair(i.override_id.as_ref(), i.override_hash.as_ref()),
                i.override_id.is_some() || i.override_hash.is_some(),
            ),
            (
                CredentialSource::Environment,
                parse_pair(i.env_id.as_ref(), i.env_hash.as_ref()),
                i.env_id.is_some() || i.env_hash.is_some(),
            ),
            (
                CredentialSource::Embedded,
                i.embedded.map(|(id, h)| (id, h.to_owned())),
                false,
            ),
        ];
        for (source, pair, present) in candidates {
            match pair {
                Some((api_id, hash)) => {
                    return Ok(Self {
                        api_id,
                        api_hash: SecretString::from(hash),
                        source,
                    });
                }
                None if present => {
                    tracing::warn!(?source, "ignoring incomplete or malformed API credentials");
                }
                None => {}
            }
        }
        Err(Error::Auth(AuthError::MissingApiCredentials))
    }
}

/// Create `dir` (and parents) and make it owner-only (0700) on Unix.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Make an existing file owner-only (0600) on Unix. Missing files are ignored.
pub fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Open a file for writing that is created owner-only (0600) from the start.
pub fn create_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    const H1: &str = "0123456789abcdef0123456789abcdef";
    const H2: &str = "fedcba9876543210fedcba9876543210";
    const H3: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn inputs() -> CredentialInputs {
        CredentialInputs {
            override_id: Some("11".into()),
            override_hash: Some(H1.into()),
            env_id: Some("22".into()),
            env_hash: Some(H2.into()),
            embedded: Some((33, H3)),
        }
    }

    #[test]
    fn embedded_values_are_well_formed_when_present() {
        // Release builds (FERROGRAM_API_ID/HASH set at build time) embed a
        // validated pair; CI builds embed nothing. Either way it is usable.
        if let Some((id, hash)) = embedded_credentials() {
            assert!(id > 0 && valid_api_hash(hash));
        }
    }

    #[test]
    fn resolution_order_override_env_embedded() {
        let c = ApiCredentials::resolve(inputs()).unwrap();
        assert_eq!((c.api_id, c.source), (11, CredentialSource::UserOverride));
        assert_eq!(c.api_hash.expose_secret(), H1);

        let mut i = inputs();
        i.override_hash = None;
        let c = ApiCredentials::resolve(i).unwrap();
        assert_eq!((c.api_id, c.source), (22, CredentialSource::Environment));

        let mut i = inputs();
        i.override_id = None;
        i.env_hash = Some("not-hex".into()); // malformed: skipped
        let c = ApiCredentials::resolve(i).unwrap();
        assert_eq!((c.api_id, c.source), (33, CredentialSource::Embedded));
        assert_eq!(c.api_hash.expose_secret(), H3);
    }

    #[test]
    fn missing_credentials_is_an_auth_error() {
        assert!(matches!(
            ApiCredentials::resolve(CredentialInputs::default()),
            Err(Error::Auth(AuthError::MissingApiCredentials))
        ));
        let i = CredentialInputs {
            env_id: Some("abc".into()),
            env_hash: Some(H1.into()),
            ..Default::default()
        };
        assert!(ApiCredentials::resolve(i).is_err());
        assert!(
            !format!("{:?}", inputs()).contains(H1),
            "Debug must not print hashes"
        );
    }

    #[test]
    fn gather_uses_keyring_override_env_and_embedded() {
        use crate::auth::secrets::MemoryStore;
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::from_env(&["x".into(), "--mock".into()]);
        cfg.config_dir = dir.path().join("cfg");
        let secrets = MemoryStore::new();
        let no_env = |_: &str| None;
        let dev_env = |k: &str| match k {
            "FERROGRAM_API_ID" => Some("22".to_owned()),
            "FERROGRAM_API_HASH" => Some(H2.to_owned()),
            _ => None,
        };

        // (c) Release build, fresh install: the embedded values.
        let c = ApiCredentials::resolve(
            cfg.gather_credential_inputs(&secrets, &no_env, Some((33, H3)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(c.source, CredentialSource::Embedded);

        // (b) Developer env vars beat the embedded values.
        let c = ApiCredentials::resolve(
            cfg.gather_credential_inputs(&secrets, &dev_env, Some((33, H3)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!((c.api_id, c.source), (22, CredentialSource::Environment));

        // (a) A saved user override beats both; the hash stays out of files.
        cfg.save_api_credentials(&secrets, 11, H1).unwrap();
        let ini = std::fs::read_to_string(cfg.config_ini()).unwrap();
        assert!(ini.contains("api_id = 11") && !ini.contains(H1));
        let c = ApiCredentials::resolve(
            cfg.gather_credential_inputs(&secrets, &dev_env, Some((33, H3)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!((c.api_id, c.source), (11, CredentialSource::UserOverride));

        // Clearing the override returns to the built-in values.
        cfg.clear_api_override(&secrets).unwrap();
        let c = ApiCredentials::resolve(
            cfg.gather_credential_inputs(&secrets, &no_env, Some((33, H3)))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(c.source, CredentialSource::Embedded);

        // Legacy plain-text hash is migrated into the keyring and scrubbed.
        std::fs::write(cfg.config_ini(), format!("api_id = 8\napi_hash = {H2}\n")).unwrap();
        let i = cfg
            .gather_credential_inputs(&secrets, &no_env, None)
            .unwrap();
        let c = ApiCredentials::resolve(i).unwrap();
        assert_eq!((c.api_id, c.source), (8, CredentialSource::UserOverride));
        assert!(
            !std::fs::read_to_string(cfg.config_ini())
                .unwrap()
                .contains(H2)
        );
    }

    #[test]
    fn screenshot_args_imply_mock() {
        let args: Vec<String> = ["x", "--screenshot", "chats", "/tmp/a.png"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let c = Config::from_env(&args);
        assert!(c.mock);
        assert_eq!(c.screenshot.unwrap().0, ScreenshotTarget::Chats);
    }
}
