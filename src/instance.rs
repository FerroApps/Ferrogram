//! Single instance: a second launch shows the first one's window and exits.
//!
//! Two tray icons would be wrong, so the first process owns an abstract Unix
//! socket (Linux). The kernel releases it when the process dies, so there is
//! no stale lock file to clean up. A second launch connects, says `show`,
//! and only exits if the owner answers `ok`: a hung process or something
//! else squatting on the name can never lock the user out of the app.
//!
//! Windows and macOS are not implemented: there every launch is primary.

use crate::remote::Remote;

/// Result of trying to become the one running instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// We own the socket; keep running.
    Primary,
    /// Another instance answered; it was told to show its window. Exit.
    Secondary,
    /// No lock available (unsupported platform, no answer): just run.
    Unlocked,
}

/// What a second launch asks the running instance to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    /// Plain launch: show the window.
    Show,
    /// `--minimized` launch (autostart twice): do nothing visible.
    Nothing,
}

impl Ask {
    fn line(self) -> &'static str {
        match self {
            Self::Show => "show\n",
            Self::Nothing => "ping\n",
        }
    }
}

/// Parse one request line from a second launch.
pub fn parse_request(line: &str) -> Option<Option<Remote>> {
    match line.trim() {
        "show" => Some(Some(Remote::Show { chat: None })),
        "ping" => Some(None),
        _ => None,
    }
}

/// Socket name for this user. Abstract sockets are per network namespace,
/// not per user, so the uid is part of the name; mock mode gets its own so a
/// demo never steals the real instance's window.
pub fn socket_name(app_id: &str, uid: u32, mock: bool) -> String {
    format!("{app_id}.instance.{uid}{}", if mock { ".mock" } else { "" })
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, channel};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{Ask, Role, parse_request};
    use crate::remote::Remote;

    const IO_TIMEOUT: Duration = Duration::from_secs(1);
    const MAX_LINE: u64 = 32;

    pub fn current_uid() -> u32 {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata("/proc/self").map_or(0, |m| m.uid())
    }

    /// The socket we own, so [`release`] can give it up (restart after an
    /// update).
    struct Held {
        fd: std::os::fd::RawFd,
        stop: Arc<AtomicBool>,
        done: Receiver<()>,
        name: String,
        sink: Arc<dyn Fn(Remote) + Send + Sync>,
    }

    static HELD: Mutex<Option<Held>> = Mutex::new(None);

    pub fn acquire(name: &str, ask: Ask, sink: Arc<dyn Fn(Remote) + Send + Sync>) -> Role {
        let Ok(addr) = SocketAddr::from_abstract_name(name.as_bytes()) else {
            return Role::Unlocked;
        };
        for _ in 0..3 {
            match UnixListener::bind_addr(&addr) {
                Ok(listener) => return serve(listener, name, sink),
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => match notify(&addr, ask) {
                    Ok(()) => return Role::Secondary,
                    Err(e) => {
                        // Owner exiting right now, or not ours: look again.
                        tracing::debug!(error = %e, "instance socket busy but silent");
                        std::thread::sleep(Duration::from_millis(150));
                    }
                },
                Err(e) => {
                    tracing::debug!(error = %e, "single-instance socket unavailable");
                    return Role::Unlocked;
                }
            }
        }
        tracing::warn!("another process holds the instance socket but does not answer");
        Role::Unlocked
    }

    fn serve(listener: UnixListener, name: &str, sink: Arc<dyn Fn(Remote) + Send + Sync>) -> Role {
        let fd = listener.as_raw_fd();
        let stop = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = channel();
        let (thread_stop, thread_sink) = (stop.clone(), sink.clone());
        let spawned = std::thread::Builder::new()
            .name("instance".into())
            .spawn(move || {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => handle(stream, thread_sink.as_ref()),
                        Err(_) if thread_stop.load(Ordering::SeqCst) => break,
                        Err(_) => std::thread::sleep(Duration::from_millis(50)),
                    }
                }
                // Dropping the listener frees the name for the next process.
                drop(listener);
                let _ = done_tx.send(());
            });
        if spawned.is_err() {
            return Role::Unlocked;
        }
        if let Ok(mut held) = HELD.lock() {
            *held = Some(Held {
                fd,
                stop,
                done,
                name: name.to_owned(),
                sink,
            });
        }
        Role::Primary
    }

    /// What [`release`] gave up, to take it back if the restart fails.
    pub struct Released {
        pub name: String,
        pub sink: Arc<dyn Fn(Remote) + Send + Sync>,
    }

    /// Give up the instance socket (before starting the updated binary, so
    /// it becomes the primary instead of showing our window). Waits up to a
    /// second for the name to be free.
    pub fn release() -> Option<Released> {
        let held = HELD.lock().ok()?.take()?;
        held.stop.store(true, Ordering::SeqCst);
        // SAFETY: `fd` belongs to the listener the instance thread still
        // owns (it only closes it after seeing `stop`); shutdown() wakes its
        // blocking accept() with an error.
        unsafe {
            libc::shutdown(held.fd, libc::SHUT_RDWR);
        }
        let freed = held.done.recv_timeout(Duration::from_secs(1)).is_ok();
        if !freed {
            tracing::warn!("instance socket did not close in time");
        }
        Some(Released {
            name: held.name,
            sink: held.sink,
        })
    }

    fn handle(mut stream: UnixStream, sink: &(dyn Fn(Remote) + Send + Sync)) {
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        let mut line = String::new();
        let read = BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line);
        if read.is_err() {
            return;
        }
        if let Some(request) = parse_request(&line) {
            if let Some(remote) = request {
                sink(remote);
            }
            let _ = stream.write_all(b"ok\n");
        }
    }

    fn notify(addr: &SocketAddr, ask: Ask) -> std::io::Result<()> {
        let mut stream = UnixStream::connect_addr(addr)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.write_all(ask.line().as_bytes())?;
        let mut reply = String::new();
        BufReader::new(&stream)
            .take(MAX_LINE)
            .read_line(&mut reply)?;
        if reply == "ok\n" {
            Ok(())
        } else {
            Err(std::io::Error::other("unexpected reply"))
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::{Released, acquire, current_uid, release};

/// Not implemented outside Linux: every launch is its own instance.
#[cfg(not(target_os = "linux"))]
pub fn acquire(
    _name: &str,
    _ask: Ask,
    _sink: std::sync::Arc<dyn Fn(Remote) + Send + Sync>,
) -> Role {
    Role::Unlocked
}

#[cfg(not(target_os = "linux"))]
pub fn current_uid() -> u32 {
    0
}

/// What `release` gave up (nothing outside Linux).
#[cfg(not(target_os = "linux"))]
pub struct Released {
    pub name: String,
    pub sink: std::sync::Arc<dyn Fn(Remote) + Send + Sync>,
}

#[cfg(not(target_os = "linux"))]
pub fn release() -> Option<Released> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse() {
        assert_eq!(
            parse_request("show\n"),
            Some(Some(Remote::Show { chat: None }))
        );
        assert_eq!(parse_request("ping"), Some(None));
        assert_eq!(parse_request("rm -rf\n"), None);
        assert_eq!(parse_request(""), None);
    }

    #[test]
    fn names_are_per_user_and_per_mode() {
        let a = socket_name("dev.ferro.ferrogram", 1000, false);
        assert_eq!(a, "dev.ferro.ferrogram.instance.1000");
        assert_ne!(a, socket_name("dev.ferro.ferrogram", 1001, false));
        assert_ne!(a, socket_name("dev.ferro.ferrogram", 1000, true));
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use std::sync::mpsc::{Sender, channel};
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        use super::super::*;

        fn unique(tag: &str) -> String {
            socket_name(
                &format!("dev.ferro.test-{}-{tag}", std::process::id()),
                0,
                false,
            )
        }

        fn sink(tx: Sender<Remote>) -> Arc<dyn Fn(Remote) + Send + Sync> {
            let tx = Mutex::new(tx);
            Arc::new(move |r| {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(r);
                }
            })
        }

        #[test]
        fn second_launch_shows_the_first_and_exits() {
            let name = unique("show");
            let (tx, rx) = channel();
            assert_eq!(acquire(&name, Ask::Show, sink(tx.clone())), Role::Primary);
            assert_eq!(acquire(&name, Ask::Show, sink(tx)), Role::Secondary);
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)),
                Ok(Remote::Show { chat: None })
            );
        }

        #[test]
        fn minimized_second_launch_changes_nothing_visible() {
            let name = unique("ping");
            let (tx, rx) = channel();
            assert_eq!(acquire(&name, Ask::Show, sink(tx.clone())), Role::Primary);
            assert_eq!(acquire(&name, Ask::Nothing, sink(tx)), Role::Secondary);
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        }

        #[test]
        fn a_silent_squatter_does_not_lock_us_out() {
            use std::os::linux::net::SocketAddrExt;
            use std::os::unix::net::{SocketAddr, UnixListener};
            let name = unique("squat");
            let addr = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
            // Listens but never answers (the backlog accepts the connect).
            let _squatter = UnixListener::bind_addr(&addr).unwrap();
            let (tx, _rx) = channel();
            assert_eq!(acquire(&name, Ask::Show, sink(tx)), Role::Unlocked);
        }
    }
}
