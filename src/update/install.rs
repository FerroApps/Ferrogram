//! Self-update of an `install.sh` install: the same files, paths and steps
//! as `install.sh` (download the tarball and its `.sha256`, verify, extract,
//! atomically replace the binary, desktop entry, icon and
//! `installed-version`).
//!
//! Nothing is replaced until everything is downloaded, verified, extracted
//! and staged next to its target; the renames then happen together and are
//! rolled back from the old copies if one fails.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use super::check::{Platform, Release, select_assets};
use super::extract::{self, Limits};
use super::http::{self, Endpoint};
use super::{CRATE, MAX_DOWNLOAD, UpdateError, Version};
use crate::config::APP_ID;

/// Binaries the tarball ships and `install.sh` installs (just the app).
pub const BINARIES: &[&str] = &[CRATE];

/// The `install.sh` paths, from its two base directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// `${XDG_BIN_HOME:-$HOME/.local/bin}`
    pub bin_dir: PathBuf,
    /// `${XDG_DATA_HOME:-$HOME/.local/share}`
    pub data_dir: PathBuf,
}

impl Layout {
    pub fn bin_path(&self, name: &str) -> PathBuf {
        self.bin_dir.join(name)
    }
    pub fn apps_dir(&self) -> PathBuf {
        self.data_dir.join("applications")
    }
    pub fn icon_dir(&self) -> PathBuf {
        self.data_dir.join("icons/hicolor/scalable/apps")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join(CRATE)
    }
    pub fn version_file(&self) -> PathBuf {
        self.state_dir().join("installed-version")
    }
    pub fn desktop_path(&self) -> PathBuf {
        self.apps_dir().join(format!("{APP_ID}.desktop"))
    }
    pub fn icon_path(&self) -> PathBuf {
        self.icon_dir().join(format!("{APP_ID}.svg"))
    }
}

/// Where an install is in its work (for the banner / Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Downloading { done: u64, total: Option<u64> },
    Verifying,
    Installing,
}

/// `install.sh`'s desktop entry rewrite: every `Exec=` line becomes the
/// absolute binary path (quoted when it has a space) followed by whatever
/// came after the shipped command (field codes).
pub fn desktop_entry(shipped: &str, bin: &Path) -> String {
    let args = shipped
        .lines()
        .find_map(|l| l.strip_prefix("Exec="))
        .map(|rest| rest.find(' ').map_or("", |i| &rest[i..]))
        .unwrap_or("");
    let bin = bin.display().to_string();
    let exec_bin = if bin.contains(' ') {
        format!("\"{bin}\"")
    } else {
        bin
    };
    let mut out = String::with_capacity(shipped.len() + 64);
    for line in shipped.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={exec_bin}{args}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// The hex digest from a `sha256sum` line (`<hex>  <name>`), checking the
/// name when there is one.
pub fn parse_checksum(text: &str, expected_name: &str) -> Result<[u8; 32], UpdateError> {
    let bad = || UpdateError::BadRelease("checksum file".into());
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(bad)?;
    let mut parts = line.split_whitespace();
    let hex = parts.next().ok_or_else(bad)?;
    if let Some(name) = parts.next()
        && name.trim_start_matches('*') != expected_name
    {
        return Err(bad());
    }
    if hex.len() != 64 {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2).ok_or_else(bad)?, 16)
            .map_err(|_| bad())?;
    }
    Ok(out)
}

/// A temporary directory removed on drop (also on every error path).
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Remove download directories left by an earlier, interrupted update.
fn sweep_stale(bin_dir: &Path) {
    let Ok(dir) = std::fs::read_dir(bin_dir) else {
        return;
    };
    let prefix = format!(".{CRATE}-update.");
    let mine = format!("{prefix}{}", std::process::id());
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name != mine {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Stream `reader` into `path`, hashing as it goes. Cancellable; capped.
pub fn download_to(
    mut reader: impl Read,
    path: &Path,
    total: Option<u64>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<[u8; 32], UpdateError> {
    let mut file = std::fs::File::create(path).map_err(|e| UpdateError::io("download", &e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut reported: u64 = 0;
    progress(Progress::Downloading { done: 0, total });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(UpdateError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(UpdateError::Network(format!("{}", e.kind()))),
        };
        done += n as u64;
        if done > MAX_DOWNLOAD || total.is_some_and(|t| done > t) {
            return Err(UpdateError::TooLarge);
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| UpdateError::io("download", &e))?;
        // About every 256 KiB: enough for a smooth bar, few messages.
        if done - reported >= 256 * 1024 {
            reported = done;
            progress(Progress::Downloading { done, total });
        }
    }
    if total.is_some_and(|t| done != t) {
        return Err(UpdateError::Network("download ended early".into()));
    }
    file.sync_all()
        .map_err(|e| UpdateError::io("download", &e))?;
    progress(Progress::Downloading { done, total });
    Ok(hasher.finalize().into())
}

/// One file to move into place.
#[derive(Debug)]
pub struct Staged {
    pub staged: PathBuf,
    pub target: PathBuf,
}

/// Move every staged file over its target, all or nothing: each existing
/// target is first linked (or copied) to `.<name>.old.<pid>`; if a rename
/// fails, the targets already replaced are restored from those copies and
/// the staged files are removed.
pub fn commit(files: &[Staged]) -> Result<(), UpdateError> {
    let pid = std::process::id();
    let backup_of = |target: &Path| -> PathBuf {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        target.with_file_name(format!(".{name}.old.{pid}"))
    };
    let cleanup_staged = |from: usize| {
        for f in &files[from..] {
            let _ = std::fs::remove_file(&f.staged);
        }
    };
    // 1. Back up what exists.
    let mut backups: Vec<Option<PathBuf>> = Vec::with_capacity(files.len());
    for f in files {
        if f.target.exists() {
            let b = backup_of(&f.target);
            let _ = std::fs::remove_file(&b);
            let made = std::fs::hard_link(&f.target, &b)
                .or_else(|_| std::fs::copy(&f.target, &b).map(|_| ()));
            if let Err(e) = made {
                for b in backups.iter().flatten() {
                    let _ = std::fs::remove_file(b);
                }
                cleanup_staged(0);
                return Err(UpdateError::io("back up", &e));
            }
            backups.push(Some(b));
        } else {
            backups.push(None);
        }
    }
    // 2. Rename everything into place.
    for (i, f) in files.iter().enumerate() {
        if let Err(e) = std::fs::rename(&f.staged, &f.target) {
            tracing::warn!(error = %e.kind(), "update: rename failed, rolling back");
            for (done, backup) in files[..i].iter().zip(&backups) {
                match backup {
                    Some(b) => {
                        let _ = std::fs::rename(b, &done.target);
                    }
                    None => {
                        let _ = std::fs::remove_file(&done.target);
                    }
                }
            }
            for b in backups[i..].iter().flatten() {
                let _ = std::fs::remove_file(b);
            }
            cleanup_staged(i);
            return Err(UpdateError::io("replace", &e));
        }
    }
    // 3. Success: drop the old copies (a running binary keeps its inode).
    for b in backups.iter().flatten() {
        let _ = std::fs::remove_file(b);
    }
    Ok(())
}

/// Write `data` to a hidden temp name next to `target` (`.<name>.new.<pid>`).
fn stage(target: &Path, data: &[u8], mode: u32) -> Result<Staged, UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target
        .parent()
        .ok_or_else(|| UpdateError::Io("bad target".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| UpdateError::io("create folder", &e))?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staged = dir.join(format!(".{name}.new.{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&staged)?;
        f.write_all(data)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        f.sync_all()
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        UpdateError::io("write", &e)
    })?;
    Ok(Staged {
        staged,
        target: target.to_path_buf(),
    })
}

/// Read a file of the extracted release (bounded).
fn read_extracted(path: &Path, limit: u64) -> Result<Vec<u8>, UpdateError> {
    let f = std::fs::File::open(path).map_err(|_| {
        UpdateError::BadRelease(format!(
            "the archive has no {}",
            path.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        ))
    })?;
    let mut out = Vec::new();
    f.take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|e| UpdateError::io("read", &e))?;
    if out.len() as u64 > limit {
        return Err(UpdateError::TooLarge);
    }
    Ok(out)
}

/// Extra switches, for tests.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Run `update-desktop-database` & co. afterwards, like `install.sh`.
    pub refresh_caches: bool,
    pub platform: Platform,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            refresh_caches: true,
            platform: Platform::current(),
        }
    }
}

/// Download, verify and install `release` into `layout`. Blocking: run it
/// on its own thread. Returns the installed version.
pub fn install(
    release: &Release,
    layout: &Layout,
    endpoint: &Endpoint,
    options: Options,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Version, UpdateError> {
    let (tarball, checksum) =
        select_assets(release, options.platform).ok_or(UpdateError::NoAsset)?;
    if tarball.size > MAX_DOWNLOAD {
        return Err(UpdateError::TooLarge);
    }
    let policy = endpoint.download_policy();
    std::fs::create_dir_all(&layout.bin_dir).map_err(|e| UpdateError::io("create folder", &e))?;
    sweep_stale(&layout.bin_dir);
    // Same file system as the binary, so the final rename is atomic.
    let tmp = TempDir(
        layout
            .bin_dir
            .join(format!(".{CRATE}-update.{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(&tmp.0);
    std::fs::create_dir(&tmp.0).map_err(|e| UpdateError::io("temp folder", &e))?;

    tracing::info!(version = %release.version, "update: downloading");
    let sum_text = http::get_small(&checksum.url, policy, 4096)?;
    let expected = parse_checksum(&String::from_utf8_lossy(&sum_text), &tarball.name)?;
    let (reader, len) = http::get_stream(&tarball.url, policy)?;
    let archive = tmp.0.join("release.tar.gz");
    let actual = download_to(reader, &archive, len, cancel, progress)?;

    progress(Progress::Verifying);
    if actual != expected {
        tracing::warn!("update: checksum mismatch, aborting");
        return Err(UpdateError::Checksum);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(UpdateError::Cancelled);
    }

    progress(Progress::Installing);
    let x = tmp.0.join("x");
    std::fs::create_dir(&x).map_err(|e| UpdateError::io("temp folder", &e))?;
    let file = std::fs::File::open(&archive).map_err(|e| UpdateError::io("read", &e))?;
    let top = x.join(extract::extract_tar_gz(
        std::io::BufReader::new(file),
        &x,
        Limits::default(),
    )?);

    let mut staged = Vec::new();
    let result = (|| {
        for name in BINARIES {
            let data = read_extracted(&top.join(name), MAX_DOWNLOAD)?;
            if !data.starts_with(b"\x7fELF") {
                return Err(UpdateError::BadRelease(format!(
                    "{name} is not a Linux binary"
                )));
            }
            staged.push(stage(&layout.bin_path(name), &data, 0o755)?);
        }
        let shipped = read_extracted(&top.join(format!("packaging/{APP_ID}.desktop")), 64 * 1024)?;
        let entry = desktop_entry(&String::from_utf8_lossy(&shipped), &layout.bin_path(CRATE));
        staged.push(stage(&layout.desktop_path(), entry.as_bytes(), 0o644)?);
        let icon = read_extracted(
            &top.join(format!("packaging/{APP_ID}.svg")),
            4 * 1024 * 1024,
        )?;
        staged.push(stage(&layout.icon_path(), &icon, 0o644)?);
        staged.push(stage(
            &layout.version_file(),
            format!("{}\n", release.version).as_bytes(),
            0o644,
        )?);
        Ok(())
    })();
    if let Err(e) = result {
        for s in &staged {
            let _ = std::fs::remove_file(&s.staged);
        }
        return Err(e);
    }
    commit(&staged)?;
    tracing::info!(version = %release.version, "update: installed");
    if options.refresh_caches {
        refresh_caches(layout);
    }
    Ok(release.version)
}

/// `install.sh`'s `refresh_caches`: best effort, errors ignored.
fn refresh_caches(layout: &Layout) {
    use std::process::{Command, Stdio};
    let run = |cmd: &str, args: &[&std::ffi::OsStr]| {
        Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    };
    let apps = layout.apps_dir();
    run("update-desktop-database", &[apps.as_os_str()]);
    let icons = layout.data_dir.join("icons/hicolor");
    run(
        "gtk-update-icon-cache",
        &["-f".as_ref(), "-t".as_ref(), icons.as_os_str()],
    );
    if !run("kbuildsycoca6", &[]) {
        run("kbuildsycoca5", &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_matches_install_sh() {
        let shipped = "[Desktop Entry]\nName=Ferrogram\nExec=ferrogram %U\nIcon=x\n";
        assert_eq!(
            desktop_entry(shipped, Path::new("/home/u/.local/bin/ferrogram")),
            "[Desktop Entry]\nName=Ferrogram\nExec=/home/u/.local/bin/ferrogram %U\nIcon=x\n"
        );
        assert_eq!(
            desktop_entry("Exec=ferrogram\n", Path::new("/home/my user/bin/ferrogram")),
            "Exec=\"/home/my user/bin/ferrogram\"\n"
        );
        // The real shipped entry.
        let real = desktop_entry(crate::desktop::DESKTOP_ENTRY, Path::new("/b/ferrogram"));
        assert!(real.lines().any(|l| l == "Exec=/b/ferrogram"));
    }

    #[test]
    fn checksum_lines_parse() {
        let hex = "ab".repeat(32);
        assert_eq!(
            parse_checksum(&format!("{hex}  f.tar.gz\n"), "f.tar.gz").unwrap(),
            [0xab; 32]
        );
        assert_eq!(
            parse_checksum(&format!("{hex} *f.tar.gz"), "f.tar.gz").unwrap(),
            [0xab; 32]
        );
        assert_eq!(parse_checksum(&hex, "f.tar.gz").unwrap(), [0xab; 32]);
        assert!(parse_checksum(&format!("{hex}  other.tar.gz"), "f.tar.gz").is_err());
        assert!(parse_checksum("abc  f.tar.gz", "f.tar.gz").is_err());
        assert!(parse_checksum(&format!("{}  f", "zz".repeat(32)), "f").is_err());
        assert!(parse_checksum("", "f").is_err());
    }

    #[test]
    fn download_hashes_caps_and_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a");
        let never = AtomicBool::new(false);
        let mut seen = Vec::new();
        let digest =
            download_to(&b"hello"[..], &path, Some(5), &never, &mut |p| seen.push(p)).unwrap();
        let expected: [u8; 32] = Sha256::digest(b"hello").into();
        assert_eq!(digest, expected);
        assert_eq!(
            seen.last(),
            Some(&Progress::Downloading {
                done: 5,
                total: Some(5)
            })
        );
        // More bytes than announced.
        assert_eq!(
            download_to(&b"hello!"[..], &path, Some(5), &never, &mut |_| {}),
            Err(UpdateError::TooLarge)
        );
        // Fewer bytes than announced.
        assert!(matches!(
            download_to(&b"hell"[..], &path, Some(5), &never, &mut |_| {}),
            Err(UpdateError::Network(_))
        ));
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            download_to(&b"hello"[..], &path, None, &cancelled, &mut |_| {}),
            Err(UpdateError::Cancelled)
        );
    }

    #[test]
    fn commit_replaces_everything_or_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"old a").unwrap();
        std::fs::write(&b, b"old b").unwrap();
        let sa = stage(&a, b"new a", 0o755).unwrap();
        let sb = stage(&b, b"new b", 0o644).unwrap();
        commit(&[sa, sb]).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"new a");
        assert_eq!(std::fs::read(&b).unwrap(), b"new b");
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }

        // The second rename fails (its staged file vanished): `a` is restored.
        let sa = stage(&a, b"newer a", 0o755).unwrap();
        let sb = stage(&b, b"newer b", 0o644).unwrap();
        std::fs::remove_file(&sb.staged).unwrap();
        let fresh = dir.path().join("fresh");
        let sf = stage(&fresh, b"x", 0o644).unwrap();
        assert!(matches!(commit(&[sf, sa, sb]), Err(UpdateError::Io(_))));
        assert_eq!(std::fs::read(&a).unwrap(), b"new a");
        assert_eq!(std::fs::read(&b).unwrap(), b"new b");
        assert!(
            !fresh.exists(),
            "a file that did not exist is removed again"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }
}
