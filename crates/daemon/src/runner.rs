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
    //
    // Each drainer writes into a shared buffer as it reads and signals
    // completion over a channel, rather than being `join`ed. Joining
    // unconditionally reintroduces an unbounded wait: a grandchild that
    // inherited the pipe keeps it open past the child's own exit, so the
    // drainer never sees EOF. Sharing the buffer means we can take whatever
    // was captured when the grace period ends — output already read is never
    // lost just because the pipe stayed open.
    let out = child
        .stdout
        .take()
        .map(|r| Drain::spawn(r, MAX_CAPTURED_OUTPUT_BYTES));
    let err = child
        .stderr
        .take()
        .map(|r| Drain::spawn(r, MAX_CAPTURED_OUTPUT_BYTES));

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The child is gone, so its own pipe ends are closed and the
                // drainers finish quickly. Budget them separately rather than
                // trusting that: an inherited pipe could hold them open.
                let stdout = out.map(|d| d.take(DRAIN_GRACE)).unwrap_or_default();
                let stderr = err.map(|d| d.take(DRAIN_GRACE)).unwrap_or_default();
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

/// How long to wait for a drainer once the child has exited. Generous for a
/// real drain (the pipe is already at EOF, so this is pure slack), but finite:
/// a grandchild holding the pipe open must not turn a "bounded" command into an
/// unbounded wait. On expiry we keep whatever was captured, so the fail-soft
/// direction is "discovery may see a truncated listing", never "the daemon
/// hangs".
const DRAIN_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// One pipe being drained on a background thread.
///
/// The buffer is shared and appended to *as bytes arrive*, so [`Drain::take`]
/// can return real output even when the drainer is still parked on a read
/// that will never hit EOF.
struct Drain {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    done: std::sync::mpsc::Receiver<()>,
}

impl Drain {
    fn spawn(mut reader: impl std::io::Read + Send + 'static, cap: usize) -> Self {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, done) = std::sync::mpsc::channel();
        let sink = std::sync::Arc::clone(&buf);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match reader.read(&mut chunk) {
                    // EOF, or an unreadable pipe: stop, but keep what we have.
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut kept) = sink.lock() {
                            // Keep reading past the cap (bytes are discarded,
                            // not buffered) so the child still sees the pipe
                            // drain and never blocks on write.
                            let room = cap.saturating_sub(kept.len());
                            kept.extend_from_slice(&chunk[..n.min(room)]);
                        }
                    }
                }
            }
            let _ = tx.send(());
        });
        Self { buf, done }
    }

    /// Waits up to `grace` for EOF, then returns everything captured so far.
    /// A drainer that panicked, disconnected, or is still blocked on an
    /// inherited pipe all degrade to "whatever the buffer holds".
    fn take(self, grace: std::time::Duration) -> Vec<u8> {
        let _ = self.done.recv_timeout(grace);
        self.buf.lock().map(|v| v.clone()).unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A grandchild that inherited stdout keeps the pipe open long after the
    /// direct child exits. Joining the drainers unconditionally turned this
    /// into an unbounded wait — a "bounded" command that blocks on a
    /// discovery subprocess's orphaned child. The wall-clock budget has to
    /// cover draining, not just waiting on the child.
    ///
    /// The background `sleep` is what holds the pipe; the shell exits at once.
    /// With the join this test takes 30s (the sleep). It must not.
    #[test]
    fn an_inherited_pipe_cannot_outlive_the_timeout() {
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg("sleep 30 & echo done");
        let started = std::time::Instant::now();
        let out = run_command_bounded(&mut command, std::time::Duration::from_secs(5));
        let elapsed = started.elapsed();

        let out = out.expect("the shell itself exits promptly, so this is not a timeout");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "run_command_bounded returned after {elapsed:?} — it waited on a pipe \
             held open by a grandchild instead of honoring its budget"
        );
    }

    /// The regression above must not cost us real output: a drainer that
    /// finishes normally is still collected in full.
    #[test]
    fn a_normal_drain_is_collected_in_full() {
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg("printf 'hello '; printf 'world'");
        let out =
            run_command_bounded(&mut command, std::time::Duration::from_secs(5)).expect("sh runs");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello world");
    }

    /// stderr is collected through the same channel path as stdout.
    #[test]
    fn stderr_is_collected_too() {
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg("printf oops 1>&2");
        let out =
            run_command_bounded(&mut command, std::time::Duration::from_secs(5)).expect("sh runs");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "oops");
        assert!(out.stdout.is_empty());
    }
}
