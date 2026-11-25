//! The updater end to end against a local HTTP server: release check with
//! ETag, download, checksum, safe extraction and atomic replacement in a
//! temporary `install.sh` layout, plus rollback on a corrupt download.
//! No real network.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use ferrogram::update::check::{self, CheckCache, Platform};
use ferrogram::update::http::Endpoint;
use ferrogram::update::install::{self, Layout, Options, Progress};
use ferrogram::update::{UpdateError, Version};
use sha2::{Digest, Sha256};

type Routes = Arc<Mutex<HashMap<String, (u16, Vec<(String, String)>, Vec<u8>)>>>;

struct Server {
    base: String,
    routes: Routes,
    /// Request lines and headers seen, lower-cased.
    seen: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Routes = Arc::default();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let (r, s) = (routes.clone(), seen.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
                s.lock().unwrap().push(first.trim().to_owned());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    s.lock().unwrap().push(line.trim().to_ascii_lowercase());
                }
                let (status, headers, body) = r.lock().unwrap().get(&path).cloned().unwrap_or((
                    404,
                    vec![],
                    b"not found".to_vec(),
                ));
                let mut out = stream;
                let mut head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(&body);
            }
        });
        Self { base, routes, seen }
    }

    fn route(&self, path: &str, status: u16, headers: &[(&str, &str)], body: &[u8]) {
        self.routes.lock().unwrap().insert(
            path.to_owned(),
            (
                status,
                headers
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
                body.to_vec(),
            ),
        );
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint::from_override(Some(format!("{}/latest", self.base)))
    }
}

fn next_version() -> Version {
    let c = Version::current();
    Version::new(c.major, c.minor + 1, 0)
}

const DESKTOP: &str = "[Desktop Entry]\nName=Ferrogram\nExec=ferrogram\nIcon=dev.ferro.ferrogram\n";
const NEW_BIN: &[u8] = b"\x7fELF new ferrogram";

/// A release tarball laid out like release.yml packages it.
fn tarball(v: Version, bin: &[u8]) -> Vec<u8> {
    let top = format!("ferrogram-v{v}-linux-x86_64");
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    let mut add = |path: String, data: &[u8], mode: u32| {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(mode);
        h.set_cksum();
        b.append_data(&mut h, path, data).unwrap();
    };
    add(format!("{top}/ferrogram"), bin, 0o755);
    add(format!("{top}/README.md"), b"readme", 0o644);
    add(
        format!("{top}/packaging/dev.ferro.ferrogram.desktop"),
        DESKTOP.as_bytes(),
        0o644,
    );
    add(
        format!("{top}/packaging/dev.ferro.ferrogram.svg"),
        b"<svg/>",
        0o644,
    );
    add(format!("{top}/licenses/x.txt"), b"ofl", 0o644);
    b.into_inner().unwrap().finish().unwrap()
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Serve `releases/latest` plus the tarball and its checksum.
fn publish(server: &Server, v: Version, archive: &[u8], checksum_of: &[u8]) {
    let name = format!("ferrogram-v{v}-linux-x86_64.tar.gz");
    let json = format!(
        r#"{{"tag_name":"v{v}","html_url":"https://github.com/FerroApps/Ferrogram/releases/tag/v{v}",
        "draft":false,"prerelease":false,"assets":[
        {{"name":"{name}","size":{},"browser_download_url":"{base}/dl/{name}"}},
        {{"name":"{name}.sha256","size":100,"browser_download_url":"{base}/dl/{name}.sha256"}}]}}"#,
        archive.len(),
        base = server.base
    );
    server.route("/latest", 200, &[("etag", "\"v1\"")], json.as_bytes());
    // Through a redirect, as GitHub does.
    server.route(
        &format!("/dl/{name}"),
        302,
        &[(
            "location",
            &format!("{}/storage/{name}?token=secret", server.base),
        )],
        b"",
    );
    server.route(&format!("/storage/{name}?token=secret"), 200, &[], archive);
    let sum = format!("{}  {name}\n", hex(&Sha256::digest(checksum_of)));
    server.route(&format!("/dl/{name}.sha256"), 200, &[], sum.as_bytes());
}

/// A temporary install.sh layout with the old version installed.
fn old_install(root: &Path) -> Layout {
    let layout = Layout {
        bin_dir: root.join("bin"),
        data_dir: root.join("share"),
    };
    std::fs::create_dir_all(&layout.bin_dir).unwrap();
    std::fs::create_dir_all(layout.state_dir()).unwrap();
    std::fs::write(layout.bin_path("ferrogram"), b"\x7fELF old").unwrap();
    std::fs::write(layout.version_file(), format!("{}\n", Version::current())).unwrap();
    layout
}

fn opts() -> Options {
    Options {
        refresh_caches: false,
        platform: Platform::LinuxX86_64,
    }
}

fn hidden_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect()
}

#[test]
fn check_uses_etag_and_finds_the_release() {
    let server = Server::start();
    let v = next_version();
    let archive = tarball(v, NEW_BIN);
    publish(&server, v, &archive, &archive);

    let (release, cache) = check::check(&server.endpoint(), &CheckCache::default()).unwrap();
    let release = release.unwrap();
    assert_eq!(release.version, v);
    assert_eq!(cache.etag.as_deref(), Some("\"v1\""));
    {
        let seen = server.seen.lock().unwrap();
        let ua = format!("user-agent: ferrogram/{}", env!("CARGO_PKG_VERSION"));
        assert!(seen.contains(&ua), "{seen:?}");
        assert!(seen.contains(&"accept: application/vnd.github+json".to_owned()));
        assert!(!seen.iter().any(|h| h.starts_with("if-none-match")));
        assert!(
            !seen
                .iter()
                .any(|h| h.starts_with("authorization") || h.starts_with("cookie"))
        );
    }

    // Second check: conditional; a 304 keeps the cached release.
    server.route("/latest", 304, &[], b"");
    let (again, cache2) = check::check(&server.endpoint(), &cache).unwrap();
    assert_eq!(again, Some(release));
    assert_eq!(cache2, cache);
    assert!(
        server
            .seen
            .lock()
            .unwrap()
            .contains(&"if-none-match: \"v1\"".to_owned())
    );

    // Rate limited: a quiet, typed error.
    server.route("/latest", 403, &[], b"{}");
    assert_eq!(
        check::check(&server.endpoint(), &cache).unwrap_err(),
        UpdateError::RateLimited
    );
}

#[test]
fn installs_atomically_into_an_install_sh_layout() {
    let server = Server::start();
    let v = next_version();
    let archive = tarball(v, NEW_BIN);
    publish(&server, v, &archive, &archive);
    let (release, _) = check::check(&server.endpoint(), &CheckCache::default()).unwrap();
    let release = release.unwrap();

    let root = tempfile::tempdir().unwrap();
    let layout = old_install(root.path());
    // A running binary keeps its inode: open it like the running process.
    let running = std::fs::File::open(layout.bin_path("ferrogram")).unwrap();
    let mut steps = Vec::new();
    let got = install::install(
        &release,
        &layout,
        &server.endpoint(),
        opts(),
        &AtomicBool::new(false),
        &mut |p| steps.push(p),
    )
    .unwrap();
    assert_eq!(got, v);
    assert_eq!(
        std::fs::read(layout.bin_path("ferrogram")).unwrap(),
        NEW_BIN
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(layout.bin_path("ferrogram"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
    drop(running);
    assert_eq!(
        std::fs::read_to_string(layout.version_file()).unwrap(),
        format!("{v}\n")
    );
    let entry = std::fs::read_to_string(layout.desktop_path()).unwrap();
    assert!(entry.contains(&format!("Exec={}", layout.bin_path("ferrogram").display())));
    assert_eq!(std::fs::read(layout.icon_path()).unwrap(), b"<svg/>");
    assert!(matches!(steps.first(), Some(Progress::Downloading { .. })));
    assert!(steps.contains(&Progress::Verifying));
    assert_eq!(steps.last(), Some(&Progress::Installing));
    assert!(
        hidden_leftovers(&layout.bin_dir).is_empty(),
        "temp files cleaned"
    );
    // The redirect (with its signed query string) was followed.
    assert!(
        server
            .seen
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("/storage/") && l.contains("token=secret"))
    );
}

#[test]
fn a_corrupt_download_changes_nothing() {
    let server = Server::start();
    let v = next_version();
    let good = tarball(v, NEW_BIN);
    let mut corrupt = good.clone();
    let last = corrupt.len() - 20;
    corrupt[last] ^= 0xff;
    // The checksum is of the good file; the server sends a corrupt one.
    publish(&server, v, &corrupt, &good);
    let (release, _) = check::check(&server.endpoint(), &CheckCache::default()).unwrap();

    let root = tempfile::tempdir().unwrap();
    let layout = old_install(root.path());
    let err = install::install(
        &release.unwrap(),
        &layout,
        &server.endpoint(),
        opts(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err, UpdateError::Checksum);
    assert_eq!(
        std::fs::read(layout.bin_path("ferrogram")).unwrap(),
        b"\x7fELF old"
    );
    assert_eq!(
        std::fs::read_to_string(layout.version_file()).unwrap(),
        format!("{}\n", Version::current())
    );
    assert!(!layout.desktop_path().exists());
    assert!(hidden_leftovers(&layout.bin_dir).is_empty());
}

#[test]
fn a_tarball_without_a_binary_or_a_cancelled_download_changes_nothing() {
    let server = Server::start();
    let v = next_version();
    let not_elf = tarball(v, b"#!/bin/sh\necho hi\n");
    publish(&server, v, &not_elf, &not_elf);
    let (release, _) = check::check(&server.endpoint(), &CheckCache::default()).unwrap();
    let release = release.unwrap();
    let root = tempfile::tempdir().unwrap();
    let layout = old_install(root.path());
    let err = install::install(
        &release,
        &layout,
        &server.endpoint(),
        opts(),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, UpdateError::BadRelease(_)), "{err:?}");
    assert_eq!(
        install::install(
            &release,
            &layout,
            &server.endpoint(),
            opts(),
            &AtomicBool::new(true),
            &mut |_| {}
        ),
        Err(UpdateError::Cancelled)
    );
    assert_eq!(
        std::fs::read(layout.bin_path("ferrogram")).unwrap(),
        b"\x7fELF old"
    );
    assert!(hidden_leftovers(&layout.bin_dir).is_empty());
}

#[test]
fn downloads_never_leave_the_allowed_hosts() {
    let server = Server::start();
    let v = next_version();
    let archive = tarball(v, NEW_BIN);
    publish(&server, v, &archive, &archive);
    let name = format!("ferrogram-v{v}-linux-x86_64.tar.gz");
    // A redirect to a host outside the allow-list is refused.
    server.route(
        &format!("/dl/{name}"),
        302,
        &[("location", "http://evil.example/x.tar.gz")],
        b"",
    );
    let (release, _) = check::check(&server.endpoint(), &CheckCache::default()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let layout = old_install(root.path());
    assert_eq!(
        install::install(
            &release.unwrap(),
            &layout,
            &server.endpoint(),
            opts(),
            &AtomicBool::new(false),
            &mut |_| {}
        ),
        Err(UpdateError::BlockedUrl)
    );
    // Without the test override, a loopback API is never contacted.
    assert_eq!(
        check::check(
            &Endpoint {
                latest_url: format!("{}/latest", server.base),
                overridden: false
            },
            &CheckCache::default()
        )
        .unwrap_err(),
        UpdateError::BlockedUrl
    );
}
