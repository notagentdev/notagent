use std::process::{Command, Stdio};

/// Open a URL or file in the platform browser/default handler.
/// This intentionally never invokes a shell. On Windows, do not use
/// `cmd /c start`: cmd.exe re-parses metacharacters (&, |, ^, ...) before
/// `start` runs, which would make attacker-controlled URLs injectable.
pub fn open_browser(target: &str) {
    let (command, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![target])
    } else if cfg!(target_os = "windows") {
        ("rundll32", vec!["url.dll,FileProtocolHandler", target])
    } else {
        ("xdg-open", vec![target])
    };

    let mut builder = Command::new(command);
    builder
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // `detached: true` — a new process group, so a terminal signal aimed at
    // notagent does not reach the launcher.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        builder.process_group(0);
    }

    // spawn reports launcher failures (for example, missing xdg-open) via an
    // error event. Browser launch is best-effort: callers still present the target
    // to the user, so keep the launcher failure from becoming a process crash.
    let Ok(mut child) = builder.spawn() else {
        return;
    };
    // `unref()` — nobody waits for the launcher. libuv reaps the child through
    // its SIGCHLD handler; Rust has no such reaper, so a thread collects the
    // exit status instead of leaving a zombie behind for the session.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}
