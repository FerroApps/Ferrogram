//! "Restart now": start the freshly installed binary with the same
//! arguments, then let the app quit through its normal path.

use std::path::Path;

/// Arguments for the new process: ours minus one-shot switches, plus
/// `--minimized` when the window is hidden (so a tray-only app stays in the
/// tray).
pub fn restart_args(args: &[String], hidden: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 1);
    let mut skip = 0usize;
    for a in args {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        match a.as_str() {
            "--minimized" | "--api-setup" | "--install-desktop-entry" => {}
            // `--screenshot <screen> <out.png>` never restarts, but be safe.
            "--screenshot" => skip = 2,
            _ => out.push(a.clone()),
        }
    }
    if hidden {
        out.push("--minimized".into());
    }
    out
}

/// Start `bin` detached (own process group, no inherited stdio).
pub fn spawn(bin: &Path, args: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn keeps_args_and_adds_minimized_when_hidden() {
        assert_eq!(restart_args(&s(&[]), false), s(&[]));
        assert_eq!(restart_args(&s(&[]), true), s(&["--minimized"]));
        assert_eq!(
            restart_args(&s(&["--minimized", "--foo"]), false),
            s(&["--foo"])
        );
        assert_eq!(
            restart_args(&s(&["--minimized"]), true),
            s(&["--minimized"]),
            "never twice"
        );
        assert_eq!(
            restart_args(
                &s(&["--api-setup", "--screenshot", "chats", "o.png", "-x"]),
                false
            ),
            s(&["-x"])
        );
    }
}
