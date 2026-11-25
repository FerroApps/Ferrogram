//! How this copy of Ferrogram was installed, which decides what "update"
//! means: replace the files ourselves (`install.sh`), tell the user the
//! `cargo install` command, or only point at the release page.
//!
//! [`detect`] is pure; [`Probe::gather`] does the file-system and
//! environment reads (canonicalised paths, markers) and is tested with
//! temporary directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::CRATE;

/// Why an install can only be notified about updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmanaged {
    /// Flatpak sandbox: the package manager owns the files.
    Flatpak,
    AppImage,
    /// Running from a cargo `target/` directory (`cargo run`).
    DevBuild,
    /// The `install.sh` location, but the directory is not writable.
    ReadOnly,
    /// A system path (`/usr/bin`, a distro package) or anywhere else.
    Other,
}

/// The install method, as shown in Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// `install.sh`: binary in `$XDG_BIN_HOME` (or `~/.local/bin`) plus the
    /// `installed-version` marker. Self-update supported.
    InstallSh {
        bin_dir: PathBuf,
        data_dir: PathBuf,
    },
    /// `cargo install` into `$CARGO_HOME/bin`: never self-updated.
    Cargo,
    Unmanaged(Unmanaged),
}

impl InstallMethod {
    pub fn can_self_update(&self) -> bool {
        matches!(self, Self::InstallSh { .. })
    }

    /// Short description for Settings.
    pub fn label(&self) -> &'static str {
        match self {
            Self::InstallSh { .. } => "install.sh (updates itself)",
            Self::Cargo => "cargo install",
            Self::Unmanaged(Unmanaged::Flatpak) => "Flatpak",
            Self::Unmanaged(Unmanaged::AppImage) => "AppImage",
            Self::Unmanaged(Unmanaged::DevBuild) => "development build",
            Self::Unmanaged(Unmanaged::ReadOnly) => "install.sh (read-only folder)",
            Self::Unmanaged(Unmanaged::Other) => "system package or manual copy",
        }
    }
}

/// Everything [`detect`] looks at, already read from the system.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// `current_exe()`, canonicalised.
    pub exe: PathBuf,
    /// `${XDG_BIN_HOME:-~/.local/bin}` (as configured, not canonicalised).
    pub bin_dir: Option<PathBuf>,
    /// `${XDG_DATA_HOME:-~/.local/share}`.
    pub data_dir: Option<PathBuf>,
    /// `<bin_dir>/ferrogram`, canonicalised, when it exists.
    pub install_sh_bin: Option<PathBuf>,
    /// `<data_dir>/ferrogram/installed-version` exists.
    pub install_sh_marker: bool,
    /// The bin directory accepts new files.
    pub bin_dir_writable: bool,
    /// `${CARGO_HOME:-~/.cargo}/bin/ferrogram`, canonicalised, when it exists.
    pub cargo_bin: Option<PathBuf>,
    pub flatpak: bool,
    pub appimage: bool,
}

/// A `…/target/<profile>/…` path: a cargo build directory.
fn in_cargo_target(exe: &Path) -> bool {
    let parts: Vec<_> = exe.components().map(|c| c.as_os_str()).collect();
    parts.windows(2).any(|w| {
        w[0] == "target"
            && (w[1] == "debug" || w[1] == "release" || w[1].to_string_lossy().contains('-'))
    })
}

/// The install method for `probe` (pure).
pub fn detect(probe: &Probe) -> InstallMethod {
    if probe.flatpak {
        return InstallMethod::Unmanaged(Unmanaged::Flatpak);
    }
    if probe.appimage {
        return InstallMethod::Unmanaged(Unmanaged::AppImage);
    }
    if probe.cargo_bin.as_deref() == Some(probe.exe.as_path()) {
        return InstallMethod::Cargo;
    }
    if in_cargo_target(&probe.exe) {
        return InstallMethod::Unmanaged(Unmanaged::DevBuild);
    }
    if let (Some(bin_dir), Some(data_dir)) = (&probe.bin_dir, &probe.data_dir)
        && probe.install_sh_marker
        && probe.install_sh_bin.as_deref() == Some(probe.exe.as_path())
    {
        return if probe.bin_dir_writable {
            InstallMethod::InstallSh {
                bin_dir: bin_dir.clone(),
                data_dir: data_dir.clone(),
            }
        } else {
            InstallMethod::Unmanaged(Unmanaged::ReadOnly)
        };
    }
    InstallMethod::Unmanaged(Unmanaged::Other)
}

/// Absolute paths from the environment only (the XDG spec ignores
/// relative values).
fn abs(v: Option<OsString>) -> Option<PathBuf> {
    v.map(PathBuf::from)
        .filter(|p| p.is_absolute() && !p.as_os_str().is_empty())
}

/// Can we create files in `dir`? (Tries for real: permission bits do not
/// tell about read-only mounts.)
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".{CRATE}.write-test.{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

impl Probe {
    /// Read the system: `exe` is `current_exe()`, `env` looks up variables.
    pub fn gather(exe: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let home = abs(env("HOME"));
        let bin_dir =
            abs(env("XDG_BIN_HOME")).or_else(|| home.as_ref().map(|h| h.join(".local/bin")));
        let data_dir =
            abs(env("XDG_DATA_HOME")).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let cargo_home = abs(env("CARGO_HOME")).or_else(|| home.as_ref().map(|h| h.join(".cargo")));
        let canon = |p: PathBuf| std::fs::canonicalize(p).ok();
        Self {
            exe: std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf()),
            install_sh_bin: bin_dir.as_ref().and_then(|d| canon(d.join(CRATE))),
            install_sh_marker: data_dir
                .as_ref()
                .is_some_and(|d| d.join(CRATE).join("installed-version").is_file()),
            bin_dir_writable: bin_dir.as_deref().is_some_and(dir_writable),
            cargo_bin: cargo_home.and_then(|d| canon(d.join("bin").join(CRATE))),
            flatpak: env("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists(),
            appimage: env("APPIMAGE").is_some(),
            bin_dir,
            data_dir,
        }
    }

    /// The running process.
    pub fn current() -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        Self::gather(&exe, &|k| std::env::var_os(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_sh_probe() -> Probe {
        Probe {
            exe: "/home/u/.local/bin/ferrogram".into(),
            bin_dir: Some("/home/u/.local/bin".into()),
            data_dir: Some("/home/u/.local/share".into()),
            install_sh_bin: Some("/home/u/.local/bin/ferrogram".into()),
            install_sh_marker: true,
            bin_dir_writable: true,
            ..Probe::default()
        }
    }

    #[test]
    fn install_sh_layout_self_updates() {
        let m = detect(&install_sh_probe());
        assert_eq!(
            m,
            InstallMethod::InstallSh {
                bin_dir: "/home/u/.local/bin".into(),
                data_dir: "/home/u/.local/share".into()
            }
        );
        assert!(m.can_self_update());
    }

    #[test]
    fn install_sh_needs_the_marker_the_same_binary_and_a_writable_dir() {
        let mut p = install_sh_probe();
        p.install_sh_marker = false;
        assert_eq!(detect(&p), InstallMethod::Unmanaged(Unmanaged::Other));

        let mut p = install_sh_probe();
        p.exe = "/usr/bin/ferrogram".into();
        assert_eq!(detect(&p), InstallMethod::Unmanaged(Unmanaged::Other));

        let mut p = install_sh_probe();
        p.bin_dir_writable = false;
        assert_eq!(detect(&p), InstallMethod::Unmanaged(Unmanaged::ReadOnly));
        assert!(!detect(&p).can_self_update());
    }

    #[test]
    fn cargo_install_is_never_self_updated() {
        let p = Probe {
            exe: "/home/u/.cargo/bin/ferrogram".into(),
            cargo_bin: Some("/home/u/.cargo/bin/ferrogram".into()),
            ..install_sh_probe()
        };
        assert_eq!(detect(&p), InstallMethod::Cargo);
        assert!(!InstallMethod::Cargo.can_self_update());
    }

    #[test]
    fn sandboxes_and_dev_builds_only_notify() {
        let p = Probe {
            flatpak: true,
            ..install_sh_probe()
        };
        assert_eq!(detect(&p), InstallMethod::Unmanaged(Unmanaged::Flatpak));
        let p = Probe {
            appimage: true,
            ..install_sh_probe()
        };
        assert_eq!(detect(&p), InstallMethod::Unmanaged(Unmanaged::AppImage));
        for exe in [
            "/home/u/src/Ferrogram/target/debug/ferrogram",
            "/home/u/src/Ferrogram/target/release/ferrogram",
            "/home/u/src/Ferrogram/target/x86_64-unknown-linux-gnu/release/ferrogram",
        ] {
            let p = Probe {
                exe: exe.into(),
                ..Probe::default()
            };
            assert_eq!(
                detect(&p),
                InstallMethod::Unmanaged(Unmanaged::DevBuild),
                "{exe}"
            );
        }
        let p = Probe {
            exe: "/usr/bin/ferrogram".into(),
            ..Probe::default()
        };
        assert_eq!(detect(&p), InstallMethod::Unmanaged(Unmanaged::Other));
    }

    #[test]
    fn gather_reads_an_install_sh_layout_through_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let bin = root.path().join("xdgbin");
        let data = root.path().join("xdgdata");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(data.join("ferrogram")).unwrap();
        std::fs::write(bin.join("ferrogram"), b"elf").unwrap();
        std::fs::write(data.join("ferrogram/installed-version"), b"0.11.0\n").unwrap();
        // The binary is reached through a symlinked directory.
        let link = root.path().join("linked-bin");
        std::os::unix::fs::symlink(&bin, &link).unwrap();
        let env = |k: &str| -> Option<OsString> {
            match k {
                "HOME" => Some(home.clone().into()),
                "XDG_BIN_HOME" => Some(link.clone().into()),
                "XDG_DATA_HOME" => Some(data.clone().into()),
                _ => None,
            }
        };
        let probe = Probe::gather(&bin.join("ferrogram"), &env);
        assert!(probe.install_sh_marker && probe.bin_dir_writable);
        assert_eq!(probe.install_sh_bin.as_deref(), Some(probe.exe.as_path()));
        assert_eq!(
            detect(&probe),
            InstallMethod::InstallSh {
                bin_dir: link.clone(),
                data_dir: data.clone()
            }
        );
        // Relative XDG values are ignored (spec), falling back to HOME.
        let env2 = |k: &str| -> Option<OsString> {
            match k {
                "HOME" => Some(home.clone().into()),
                "XDG_BIN_HOME" => Some("relative/bin".into()),
                _ => None,
            }
        };
        let p2 = Probe::gather(&bin.join("ferrogram"), &env2);
        assert_eq!(p2.bin_dir, Some(home.join(".local/bin")));
        assert_eq!(detect(&p2), InstallMethod::Unmanaged(Unmanaged::Other));
    }

    #[test]
    fn gather_finds_cargo_home() {
        let root = tempfile::tempdir().unwrap();
        let cargo = root.path().join("cargo");
        std::fs::create_dir_all(cargo.join("bin")).unwrap();
        std::fs::write(cargo.join("bin/ferrogram"), b"elf").unwrap();
        let env =
            |k: &str| -> Option<OsString> { (k == "CARGO_HOME").then(|| cargo.clone().into()) };
        let probe = Probe::gather(&cargo.join("bin/ferrogram"), &env);
        assert_eq!(detect(&probe), InstallMethod::Cargo);
    }
}
