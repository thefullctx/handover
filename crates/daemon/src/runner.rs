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

/// Hard cap on the stdout/stderr a bounded command may buffer. The child is
/// drained to EOF either way (so it never blocks on a full pipe), but only
/// this much is kept — a discovery command that prints megabytes must not be
/// able to grow the daemon's memory without bound.
const MAX_CAPTURED_OUTPUT_BYTES: usize = 256 * 1024;

/// Spawns a command with piped stdout/stderr and a hard wall-clock budget.
/// Returns `None` on spawn failure, timeout (the child and its process group
/// are killed), or wait error; `Some(Output)` otherwise, mirroring
/// `Command::output`.
///
/// Pipes are drained concurrently via background threads so the child never
/// blocks on a full pipe buffer (the bug that `GenericCommandAgent` already
/// solves with `drain_capped`).
pub(crate) fn run_command_bounded(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    // Own process group, exactly like the agent runner: on timeout we signal
    // `-pid`, so a discovery command that spawned children takes them down
    // with it instead of leaking orphans. `Child::kill` alone only reaps the
    // direct child.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // Drain stdout and stderr in background threads so the child never
    // blocks on a full OS pipe buffer (~64KB). Without this, a command
    // that writes more than the buffer would deadlock: the child waits
    // on write() and the parent waits on try_wait().
    let stdout_h = child
        .stdout
        .take()
        .map(|r| std::thread::spawn(move || drain_capped(r, MAX_CAPTURED_OUTPUT_BYTES)));
    let stderr_h = child
        .stderr
        .take()
        .map(|r| std::thread::spawn(move || drain_capped(r, MAX_CAPTURED_OUTPUT_BYTES)));

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
                    kill_child_tree(&mut child);
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => {
                kill_child_tree(&mut child);
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Reads a pipe to EOF, keeping at most `cap` bytes. Reading continues past
/// the cap (bytes are discarded, not buffered) so the child still sees the
/// pipe drain and never blocks on write.
fn drain_capped(mut reader: impl std::io::Read, cap: usize) -> Vec<u8> {
    let mut kept: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
}

/// Kill the bounded command's whole process tree (see the `process_group(0)`
/// above). Mirrors the agent runner: signal the group first, then fall back
/// to killing the direct child.
fn kill_child_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // Negative PID = process group id. libc directly: shelling out to
        // `kill` breaks under Finder-launched restricted PATHs.
        unsafe {
            let _ = libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
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
