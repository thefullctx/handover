//! Bounded subprocess helpers for session discovery.
//!
//! Every subprocess gets a hard wall-clock budget and fails soft — a hung
//! cli-list must never stall a palette open or the daemon lock.

use std::path::PathBuf;

/// Hard cap for a session cli-list command (e.g. `hermes sessions list`).
/// Discovery is a status read — a hung listing fails soft, never blocks.
const CLI_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolves a cli-list program to an absolute path. Finder-launched apps
/// inherit a restricted PATH (typically just `/usr/bin:/bin`), so a bare
/// name like `hermes` — installed in `~/.local/bin` — would silently fail to
/// spawn and discovery would return nothing. Absolute paths pass through;
/// bare names are looked up in the inherited PATH plus the common macOS
/// install locations (user bin dirs and Homebrew). Falls back to the bare
/// name so `Command::new` reports the real spawn error.
pub(crate) fn resolve_cli_program(program: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join("bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/sbin"));
    for dir in dirs {
        let candidate = dir.join(program);
        if !candidate.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = candidate.metadata() {
                if meta.permissions().mode() & 0o111 == 0 {
                    continue; // present but not executable — keep looking
                }
            }
        }
        return candidate.to_string_lossy().into_owned();
    }
    program.to_string()
}

/// Spawns a command with piped stdout/stderr and a hard wall-clock budget.
/// Returns `None` on spawn failure, timeout (the child is killed), or wait
/// error; `Some(Output)` otherwise, mirroring `Command::output`.
///
/// Pipes are drained concurrently via background threads so the child never
/// blocks on a full pipe buffer (the bug that `GenericCommandAgent` already
/// solves with `drain_capped`).
pub(crate) fn run_command_bounded(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // Drain stdout and stderr in background threads so the child never
    // blocks on a full OS pipe buffer (~64KB). Without this, a command
    // that writes more than the buffer would deadlock: the child waits
    // on write() and the parent waits on try_wait().
    let stdout_h = child.stdout.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = r;
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_h = child.stderr.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = r;
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_h.and_then(|h| h.join().ok()).unwrap_or_default();
                let stderr = stderr_h.and_then(|h| h.join().ok()).unwrap_or_default();
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Runs a session cli-list command with a hard [`CLI_LIST_TIMEOUT`] budget.
/// Returns `None` on spawn failure or timeout (the child is killed);
/// `Some(Output)` otherwise — including non-zero exits, which the caller
/// treats fail-soft.
pub(crate) fn run_cli_list(program: &str, args: &[String]) -> Option<std::process::Output> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    run_command_bounded(&mut command, CLI_LIST_TIMEOUT)
}
