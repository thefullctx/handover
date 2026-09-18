//! The generic command agent.
//!
//! Users can configure any CLI command as an agent, e.g.
//! `my-agent --prompt "{PROMPT}"`. This keeps Handover independent of a
//! fixed list of agents — a new agent works without an Handover update.
//!
//! Substitution rules:
//! * `{PROMPT}` — replaced with the rendered prompt (respecting how the user
//!   quoted the placeholder: `"{PROMPT}"`, `'{PROMPT}'` or bare `{PROMPT}`)
//! * `{PROMPT_FILE}` — replaced with the path of a temp file containing the
//!   prompt (same quoting rules apply)
//! * if neither placeholder is present, the prompt is piped to the command
//!   on stdin (so `sh -c "cat"` style sinks work)

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use handover_config::AgentConfig;
use handover_core::agent::{
    Agent, AgentError, AgentMeta, AgentRequest, AgentStatus, OutputChannel, OutputSink, SendReceipt,
};

/// Maximum bytes of stdout/stderr retained in a `SendReceipt`.
/// Larger streams are still fully drained (to avoid pipe deadlock) but truncated
/// for storage with a short prefix note.
const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

pub struct GenericCommandAgent {
    pub config: AgentConfig,
}

impl GenericCommandAgent {
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }
}

/// Private, mode-restricted prompt temp file. Cleaned up on drop for every
/// exit path (success, timeout, spawn failure, wait error).
struct PromptTempFile {
    path: PathBuf,
}

impl PromptTempFile {
    fn create(contents: &str) -> Result<Self, std::io::Error> {
        let dir = prompt_temp_dir()?;
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }

        let path = dir.join(format!("prompt-{}.txt", uuid::Uuid::new_v4()));
        write_private_file(&path, contents.as_bytes())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PromptTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Directory for prompt temp files: user cache/local data when available,
/// otherwise a private subdirectory under the system temp dir. Never the bare
/// world-shared `/tmp` root.
///
/// `HANDOVER_PROMPT_CACHE_DIR` overrides the location entirely — used by
/// tests and sandboxed/CI environments that must not write into the user's
/// real cache directory (and to make prompt-cache behavior deterministic).
fn prompt_temp_dir() -> Result<PathBuf, std::io::Error> {
    if let Some(dir) = std::env::var_os("HANDOVER_PROMPT_CACHE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::cache_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| std::env::temp_dir().join("handover-private"));
    Ok(base.join("handover").join("prompts"))
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

impl Agent for GenericCommandAgent {
    fn meta(&self) -> AgentMeta {
        AgentMeta {
            id: self.config.id.clone(),
            name: self.config.name.clone(),
            description: self
                .config
                .description
                .clone()
                .unwrap_or_else(|| "Custom command agent".to_string()),
            kind: "command".to_string(),
            config_summary: Some(self.config.command.clone()),
            demo: self.config.demo,
        }
    }

    fn detect(&self) -> Result<AgentStatus, AgentError> {
        let command = self.config.command.trim();
        if command.is_empty() {
            return Ok(AgentStatus {
                available: false,
                detail: "No command configured for this agent.".to_string(),
                running: false,
                provider_down: None,
            });
        }
        match first_program(command) {
            Some(program) if program_exists(&program).is_some() => Ok(AgentStatus {
                available: true,
                detail: format!("Command `{program}` found on PATH"),
                running: false,
                provider_down: None, // filled by the daemon's live-process check
            }),
            Some(program) => Ok(AgentStatus {
                available: false,
                detail: format!("Command `{program}` not found on PATH"),
                running: false,
                provider_down: None,
            }),
            None => Ok(AgentStatus {
                available: false,
                detail: "Could not parse the configured command.".to_string(),
                running: false,
                provider_down: None,
            }),
        }
    }

    fn send(
        &self,
        request: &AgentRequest,
        stream: Option<OutputSink>,
    ) -> Result<SendReceipt, AgentError> {
        // Fresh send: the plain command. The session-aware adapter
        // (SessionAgent) chooses between this and the resume command.
        run_command(
            &self.config,
            self.config.command.trim(),
            &request.prompt,
            None,
            stream,
        )
    }
}

/// Runs a shell command as an agent handoff: substitutes `{PROMPT}` /
/// `{PROMPT_FILE}` (and `{SESSION}` when given), spawns under `sh -c` in its
/// own process group, drains stdout/stderr concurrently with streaming, and
/// enforces the configured timeout by killing the whole tree.
///
/// Shared by the generic command agent (fresh sends) and the session agent
/// (resume-by-id sends), so both keep the same timeout/streaming/process-group
/// semantics.
pub(crate) fn run_command(
    config: &AgentConfig,
    command: &str,
    prompt: &str,
    session: Option<&str>,
    stream: Option<OutputSink>,
) -> Result<SendReceipt, AgentError> {
    let start = Instant::now();
    let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(120));
    let name = config.name.clone();
    let id = config.id.clone();

    // Private prompt temp file (0600). RAII Drop removes it on every
    // exit path — including spawn failure and timeout.
    // Note: the demo agent may still write handoffs to /tmp by design
    // (see its configured command); that is demo-only and separate from
    // this prompt file.
    let prompt_file = PromptTempFile::create(prompt).map_err(|e| AgentError::Failed {
        agent: name.clone(),
        detail: format!("could not write prompt file: {e}"),
    })?;
    let prompt_path = prompt_file.path();

    let uses_prompt_placeholder = command.contains("{PROMPT}");
    let uses_file_placeholder = command.contains("{PROMPT_FILE}");
    let substituted = substitute(command, prompt, &prompt_path.to_string_lossy(), session);
    let use_stdin = !uses_prompt_placeholder && !uses_file_placeholder;

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&substituted)
        .stdin(if use_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &config.working_dir {
        cmd.current_dir(dir);
    }
    cmd.envs(&config.env);

    // The bundled demo agent's sink path is injected rather than baked into
    // its command: it must land in the user's own home (owner-only), never a
    // hardcoded world-readable location like /tmp. Only set when the agent's
    // command actually references it, so real agents never see the variable.
    if substituted.contains("HANDOVER_DEMO_SINK") {
        if let Some(home) = dirs::home_dir() {
            let sink = home.join(".handover/demo-handoff.txt");
            // The dir may not exist yet — create it so tee can write.
            if let Some(parent) = sink.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            cmd.env("HANDOVER_DEMO_SINK", sink);
        }
    }

    // Own process group so timeout can kill the whole tree (shell + agent),
    // not just the outer `sh -c` wrapper.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| AgentError::Failed {
        agent: name.clone(),
        detail: format!("could not start command: {e}"),
    })?;

    if use_stdin {
        if let Some(mut stdin) = child.stdin.take() {
            let buf = prompt.as_bytes().to_vec();
            // Write in a thread so a child that never reads stdin cannot
            // block us past the timeout.
            std::thread::spawn(move || {
                let _ = stdin.write_all(&buf);
            });
        }
    }

    // Drain stdout/stderr concurrently while waiting. Reading only after
    // exit deadlocks when a verbose agent fills the OS pipe buffer. Each
    // chunk is also forwarded to the optional stream sink (live output).
    let stdout_h = child
        .stdout
        .take()
        .map(|r| drain_capped(r, stream.clone(), OutputChannel::Stdout));
    let stderr_h = child
        .stderr
        .take()
        .map(|r| drain_capped(r, stream, OutputChannel::Stderr));

    enum WaitOutcome {
        Done(std::process::ExitStatus),
        TimedOut,
        WaitError(std::io::Error),
    }

    let outcome = loop {
        match child.try_wait() {
            Ok(Some(status)) => break WaitOutcome::Done(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    kill_child_tree(&mut child);
                    let _ = child.wait();
                    break WaitOutcome::TimedOut;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                kill_child_tree(&mut child);
                let _ = child.wait();
                break WaitOutcome::WaitError(e);
            }
        }
    };

    // Join drain threads after the child is reaped so pipes close and
    // readers finish (including on timeout).
    let stdout = join_drain(stdout_h);
    let stderr = join_drain(stderr_h);

    // prompt_file Drop cleans the temp file as this function returns
    match outcome {
        WaitOutcome::Done(status) => {
            let ok = status.success();
            let detail = if ok {
                "Handed off successfully".to_string()
            } else {
                format!("Exited with status {}", status.code().unwrap_or(-1))
            };
            Ok(SendReceipt {
                ok,
                agent_id: id,
                agent_name: name,
                detail,
                stdout: Some(stdout),
                stderr: Some(stderr),
                duration_ms: start.elapsed().as_millis() as u64,
                session_id: None,
            })
        }
        WaitOutcome::TimedOut => Err(AgentError::Timeout {
            agent: name,
            secs: timeout.as_secs(),
        }),
        WaitOutcome::WaitError(e) => Err(AgentError::Failed {
            agent: name,
            detail: format!("could not wait for command: {e}"),
        }),
    }
}

/// Continuously read a pipe, keeping at most `MAX_CAPTURED_OUTPUT_BYTES` and
/// forwarding every chunk to `sink` (live streaming) as it arrives. Always
/// drains fully so the child cannot block on a full pipe buffer.
///
/// Multi-byte UTF-8 characters that straddle a chunk boundary are carried
/// over (up to 3 trailing bytes) so the live stream — and the stored
/// receipt — never show a spurious replacement character.
fn drain_capped(
    mut reader: impl Read + Send + 'static,
    sink: Option<OutputSink>,
    channel: OutputChannel,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut tmp = [0u8; 8192];
        let mut carry: Vec<u8> = Vec::with_capacity(3);
        let mut truncated = false;
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => {
                    // End of stream: flush any incomplete trailing bytes.
                    if !carry.is_empty() {
                        if let Some(sink) = &sink {
                            sink(channel, &String::from_utf8_lossy(&carry));
                        }
                        kept.extend_from_slice(&carry);
                    }
                    break;
                }
                Ok(n) => {
                    let mut buf = Vec::with_capacity(carry.len() + n);
                    buf.extend_from_slice(&carry);
                    buf.extend_from_slice(&tmp[..n]);
                    // Split off any trailing incomplete UTF-8 sequence (kept
                    // for the next chunk); the prefix is fully decodable.
                    let split = utf8_complete_prefix_len(&buf);
                    if let Some(sink) = &sink {
                        sink(channel, &String::from_utf8_lossy(&buf[..split]));
                    }
                    if kept.len() < MAX_CAPTURED_OUTPUT_BYTES {
                        let room = MAX_CAPTURED_OUTPUT_BYTES - kept.len();
                        let take = split.min(room);
                        kept.extend_from_slice(&buf[..take]);
                        if take < split {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                    carry.clear();
                    carry.extend_from_slice(&buf[split..]);
                }
                Err(_) => break,
            }
        }
        let body = String::from_utf8_lossy(&kept).into_owned();
        if truncated {
            format!("[output truncated to {MAX_CAPTURED_OUTPUT_BYTES} bytes]\n{body}")
        } else {
            body
        }
    })
}

/// Length of the longest prefix of `buf` that ends on a complete UTF-8
/// character boundary — i.e. everything except an unfinished trailing
/// multi-byte sequence (at most 3 bytes).
fn utf8_complete_prefix_len(buf: &[u8]) -> usize {
    if std::str::from_utf8(buf).is_ok() {
        return buf.len();
    }
    // Walk back over continuation bytes to the lead byte of the trailing
    // (incomplete) sequence; everything before it is complete.
    let mut i = buf.len();
    while i > 0 && (buf[i - 1] & 0xC0) == 0x80 {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    i - 1
}

fn join_drain(handle: Option<std::thread::JoinHandle<String>>) -> String {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// Kill the agent process tree. On Unix the child was started in its own
/// process group (`process_group(0)`), so signalling `-pid` reaps the shell
/// wrapper and any descendants; a direct `Child::kill` is the fallback.
fn kill_child_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // Negative PID = process group id (same as the shell after process_group(0)).
        // Use libc directly: shelling out to the `kill` binary breaks under
        // Finder-launched restricted PATHs.
        unsafe {
            let _ = libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

/// Substitutes `{PROMPT}`, `{PROMPT_FILE}` and (optionally) `{SESSION}` into
/// a user command, honoring how the user quoted the placeholder:
/// * `"{PROMPT}"` → double-quote-escaped content, quotes kept as written
/// * `'{PROMPT}'` → single-quote-escaped content, quotes kept as written
/// * bare `{PROMPT}` → double-quote-escaped content wrapped in quotes
///
/// `{SESSION}` follows the same rules; when `session` is `None`, any
/// `{SESSION}` placeholder is left untouched (fresh sends never contain it).
fn substitute(command: &str, prompt: &str, prompt_file: &str, session: Option<&str>) -> String {
    let dq_prompt = double_quote_escape(prompt);
    let sq_prompt = single_quote_escape(prompt);
    let dq_file = double_quote_escape(prompt_file);
    let sq_file = single_quote_escape(prompt_file);
    let dq_session = session.map(double_quote_escape);
    let sq_session = session.map(single_quote_escape);

    // Single-pass scan: the command is walked left-to-right once and each
    // placeholder is replaced by writing to a SEPARATE output buffer. The
    // substituted text is therefore never rescanned — a prompt containing a
    // literal `{PROMPT}` cannot be substituted again (the previous
    // sequential-str::replace implementation re-scanned inserted prompt
    // content, which let prompt text escape the protective quotes).
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    'scan: while !rest.is_empty() {
        // Longest (quoted) form first, then bare — a match writes the
        // substitution to `out` and resumes scanning strictly AFTER it.
        for (pattern, replacement) in [
            ("\"{PROMPT_FILE}\"", format!("\"{dq_file}\"")),
            ("'{PROMPT_FILE}'", format!("'{sq_file}'")),
            ("\"{PROMPT}\"", format!("\"{dq_prompt}\"")),
            ("'{PROMPT}'", format!("'{sq_prompt}'")),
            (
                "\"{SESSION}\"",
                format!("\"{}\"", dq_session.as_deref().unwrap_or_default()),
            ),
            (
                "'{SESSION}'",
                format!("'{}'", sq_session.as_deref().unwrap_or_default()),
            ),
            ("{PROMPT_FILE}", format!("\"{dq_file}\"")),
            ("{PROMPT}", format!("\"{dq_prompt}\"")),
            (
                "{SESSION}",
                format!("\"{}\"", dq_session.as_deref().unwrap_or_default()),
            ),
        ] {
            if let Some(tail) = rest.strip_prefix(pattern) {
                match (pattern, session) {
                    // Fresh sends never contain {SESSION}; the placeholders
                    // stay literal when there is no session to substitute.
                    ("\"{SESSION}\"", None) | ("'{SESSION}'", None) | ("{SESSION}", None) => {
                        out.push_str(pattern);
                    }
                    _ => out.push_str(&replacement),
                }
                rest = tail;
                continue 'scan;
            }
        }
        // Not a placeholder start — copy one character (not byte: must stay
        // on char boundaries for non-ASCII commands).
        let mut chars = rest.chars();
        out.push(chars.next().unwrap());
        rest = chars.as_str();
    }
    out
}

/// Escapes a string for safe use inside double quotes.
fn double_quote_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

/// Escapes a string for safe use inside single quotes.
fn single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// A tiny shell-like tokenizer for extracting the program name from a
/// command line (handles single/double quotes).
fn shlex_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_token = true;
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        break;
                    }
                    current.push(c2);
                }
            }
            '"' => {
                in_token = true;
                for c2 in chars.by_ref() {
                    if c2 == '"' {
                        break;
                    }
                    current.push(c2);
                }
            }
            c if c.is_whitespace() => {
                if in_token {
                    out.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                in_token = true;
                current.push(c);
            }
        }
    }
    if in_token || !current.is_empty() {
        out.push(current);
    }
    out
}

/// Returns the first real program in a command, skipping wrappers like
/// `sh -c` / `env` and taking the first word of a shell string.
/// Public so the daemon can reuse the same extraction for its live-process
/// status check (`agent_process_running`).
///
/// Shell-string prefixes are skipped so `detect()` and the status light see
/// the AGENT, not shell machinery: leading variable assignments (`FOO=1 hermes`),
/// `export`/`unset` statements (`export FOO=1; hermes`), and `cd` commands
/// (`cd ~/proj && hermes`) all resolve to `hermes`. When a separator is
/// involved, the LAST segment wins (what actually runs the agent).
///
/// KNOWN LIMIT: the scan is syntactic, not a shell simulator. A program token
/// that is actually a quoted *string argument* is still treated as a program
/// (`printf "hermes"` reads as `hermes`). Commands that fancy are rare as
/// agent launchers, and the failure mode is only a wrong status light /
/// availability probe — never a wrong command execution (send always runs the
/// configured command verbatim under `sh -c`).
pub fn first_program(command: &str) -> Option<String> {
    let mut current = command;
    let mut result: Option<String> = None;
    // Walk statement-by-statement across unquoted `;`, `&&`, `||`, `|`, `&`
    // and newlines. Each segment that looks like it launches something
    // updates `result`; the loop ends on the last one (shell semantics: the
    // final command's status is the string's). Separators inside quotes are
    // literal text — `hermes -z "do a && do b"` is one command, not two.
    loop {
        let (cut, sep_len) = find_unquoted_separator(current).unwrap_or((current.len(), 0));
        let segment = &current[..cut];
        if let Some(program) = first_program_in_segment(segment) {
            result = Some(program);
        }
        if cut >= current.len() {
            break;
        }
        current = &current[cut + sep_len..];
    }
    result
}

/// Byte index and length of the first shell separator that is OUTSIDE single
/// or double quotes (`;`, `&&`, `&`, `||`, `|`, newline). Quotes and
/// backslash-escapes are tracked so a separator inside a quoted argument
/// (`--prompt "a && b"`) is never mistaken for a command boundary.
fn find_unquoted_separator(s: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut single = false;
    let mut double = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'\\' if !single => i += 1, // skip the escaped character
            b';' | b'\n' if !single && !double => return Some((i, 1)),
            b'&' | b'|' if !single && !double => {
                let doubled = bytes.get(i + 1) == Some(&bytes[i]);
                return Some((i, if doubled { 2 } else { 1 }));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// First program of ONE statement (no separators): strips leading variable
/// assignments and `export`/`unset`/`exec`/`cd`/wrapper prefixes, then takes
/// the first token of what remains.
fn first_program_in_segment(segment: &str) -> Option<String> {
    let tokens = shlex_split(segment);
    let mut i = 0;
    // Leading VAR=value assignments: `FOO=1 hermes`, `A=1 B=2 hermes`.
    while i < tokens.len() && is_leading_assignment(&tokens[i]) {
        i += 1;
    }
    // `export FOO=1; hermes` / `unset FOO; hermes`: the statement launches
    // nothing — skip it and any assignments that follow in the same one.
    if matches!(tokens.get(i).map(String::as_str), Some("export") | Some("unset")) {
        i += 1;
        while i < tokens.len() && is_leading_assignment(&tokens[i]) {
            i += 1;
        }
    }
    // `cd dir` never launches the agent (its argument is a directory);
    // only a later `&&`/`;` segment can contain the program.
    if matches!(tokens.get(i).map(String::as_str), Some("cd")) {
        return None;
    }
    // `exec hermes …` runs hermes in the same process — transparent.
    if matches!(tokens.get(i).map(String::as_str), Some("exec")) {
        i += 1;
        while i < tokens.len() && is_leading_assignment(&tokens[i]) {
            i += 1;
        }
    }
    // `sh -c` / `bash -c` / `zsh -c` / `env` wrappers: recurse into the
    // wrapper's argument (for -c) or skip past the assignments (for env).
    let is_wrapper = matches!(
        tokens.get(i).map(String::as_str),
        Some("sh") | Some("bash") | Some("zsh") | Some("env")
    );
    if is_wrapper {
        // Skip wrapper flags (`env -i`, `sh -l -c`, ...) to find the target —
        // but stop ON `-c` (it consumes the next token as the shell string).
        let mut j = i + 1;
        while j < tokens.len() && tokens[j].starts_with('-') && tokens[j] != "-c" {
            j += 1;
        }
        match tokens.get(j).map(String::as_str) {
            Some("-c") => {
                // Recurse into the shell string: handles nesting like
                // `sh -c 'sh -c "hermes …"'` and `sh -c 'env FOO=1 hermes'`.
                if let Some(inner) = tokens.get(j + 1) {
                    return first_program(inner);
                }
            }
            Some(_) => {
                // `env VAR=1 hermes …`: skip the env assignments, take the
                // program (first non-assignment after `env`).
                let mut k = j;
                while k < tokens.len() && is_leading_assignment(&tokens[k]) {
                    k += 1;
                }
                return tokens
                    .get(k)
                    .map(|t| t.split_whitespace().next().unwrap_or("").to_string());
            }
            None => return None,
        }
    }
    tokens
        .get(i)
        .map(|t| t.split_whitespace().next().unwrap_or("").to_string())
}

/// `FOO=1`, `FOO=`, `FOO="a b"` (post-shlex) — but not a bare word, a path
/// with an equals inside a name that has no `=` before the first `/`, or a
/// single `-` flag.
fn is_leading_assignment(token: &str) -> bool {
    if token.starts_with('-') {
        return false;
    }
    match token.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

fn program_exists(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let path = Path::new(program);
        return if path.is_file() {
            Some(path.to_path_buf())
        } else {
            None
        };
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use handover_core::action::builtin_actions;
    use handover_core::agent::{AgentRequest, AgentStatus, OutputSink};
    use handover_core::capture::{Capture, SourceKind};
    use std::sync::{Arc, Mutex};

    fn request_for(prompt: &str) -> AgentRequest {
        AgentRequest {
            capture: Capture::text(SourceKind::Manual, "the error", None),
            action: builtin_actions().remove(0),
            prompt: prompt.to_string(),
            session: None,
        }
    }

    fn agent_with(command: &str, timeout: Option<u64>) -> GenericCommandAgent {
        GenericCommandAgent::new(AgentConfig {
            id: "test".into(),
            name: "Test Agent".into(),
            kind: handover_config::AgentKind::Command,
            command: command.into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: timeout,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: None,
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        })
    }

    #[test]
    fn detect_finds_real_program() {
        let agent = agent_with("cat --prompt {PROMPT}", None);
        let status: AgentStatus = agent.detect().unwrap();
        assert!(status.available, "cat should be found: {}", status.detail);
    }

    #[test]
    fn detect_reports_missing_program() {
        let agent = agent_with("definitely-not-a-real-agent-xyz --prompt {PROMPT}", None);
        let status: AgentStatus = agent.detect().unwrap();
        assert!(!status.available);
    }

    #[test]
    fn detects_program_inside_sh_c_wrapper() {
        let agent = agent_with("sh -c 'tee /tmp/x'", None);
        let status: AgentStatus = agent.detect().unwrap();
        assert!(status.available, "tee should be found: {}", status.detail);
    }

    // -- first_program: shell-string prefixes (regression: bug where
    //    `export FOO=1; hermes …` resolved to `export` and reported the
    //    agent available regardless of whether its binary existed) --------

    #[test]
    fn first_program_sees_through_leading_assignment() {
        assert_eq!(
            first_program("FOO=1 hermes -z \"{PROMPT}\"").as_deref(),
            Some("hermes")
        );
        assert_eq!(
            first_program("A=1 B=2 HANDOVER_X=y omp -p \"{PROMPT}\"").as_deref(),
            Some("omp")
        );
    }

    #[test]
    fn first_program_sees_through_export_statement() {
        assert_eq!(
            first_program("export FOO=1; hermes -z \"{PROMPT}\"").as_deref(),
            Some("hermes")
        );
        assert_eq!(
            first_program("export A=1 B=2 && codex exec \"{PROMPT}\"").as_deref(),
            Some("codex")
        );
        // The last launching segment wins (shell semantics).
        assert_eq!(
            first_program("hermes -z 'x'; codex exec \"y\"").as_deref(),
            Some("codex")
        );
    }

    #[test]
    fn first_program_sees_through_cd_and_exec() {
        assert_eq!(
            first_program("cd ~/proj && hermes -z \"{PROMPT}\"").as_deref(),
            Some("hermes")
        );
        assert_eq!(first_program("cd /tmp").as_deref(), None);
        assert_eq!(
            first_program("exec env HOME=/x hermes -z \"p\"").as_deref(),
            Some("hermes")
        );
    }

    #[test]
    fn first_program_sees_through_env_wrapper() {
        // `env` WITHOUT -c: skip assignments after it.
        assert_eq!(
            first_program("env FOO=1 hermes -z \"{PROMPT}\"").as_deref(),
            Some("hermes")
        );
        // Bare `env` alone is not a program — fail closed so neither detect()
        // nor the status light pgreps shell machinery.
        assert_eq!(first_program("env").as_deref(), None);
    }

    #[test]
    fn first_program_ignores_separators_inside_quotes() {
        // The `&&` is prompt TEXT, not a command boundary.
        assert_eq!(
            first_program("hermes -z \"do a && do b; then c\"").as_deref(),
            Some("hermes")
        );
        assert_eq!(
            first_program("sh -c 'printf \"x; y\"'").as_deref(),
            Some("printf")
        );
    }

    #[test]
    fn first_program_handles_pipes_and_newlines() {
        assert_eq!(
            first_program("echo hi | tee log; hermes -z \"{PROMPT}\"").as_deref(),
            Some("hermes")
        );
        assert_eq!(
            first_program("export A=1\nhermes -z \"{PROMPT}\"").as_deref(),
            Some("hermes")
        );
    }

    #[test]
    fn first_program_still_handles_plain_commands() {
        assert_eq!(first_program("hermes").as_deref(), Some("hermes"));
        assert_eq!(
            first_program("/usr/local/bin/hermes -z \"{PROMPT}\"").as_deref(),
            Some("/usr/local/bin/hermes")
        );
        assert_eq!(
            first_program("sh -c 'tee /tmp/x'").as_deref(),
            Some("tee")
        );
        assert_eq!(first_program("").as_deref(), None);
    }

    #[test]
    fn detect_unavailable_when_only_shell_machinery_parses() {
        // Regression: `export FOO=1; <missing-binary> …` used to read as
        // `export` (always present) → wrongly "available". The export is now
        // skipped and the missing binary is what gets probed.
        let agent = agent_with("export FOO=1; definitely-not-a-real-agent-xyz -z \"{PROMPT}\"", None);
        let status: AgentStatus = agent.detect().unwrap();
        assert!(!status.available, "{}", status.detail);
        assert!(status.detail.contains("definitely-not-a-real-agent-xyz"));
    }

    #[test]
    fn sends_prompt_via_stdin() {
        // No {PROMPT} placeholder → the prompt is piped to stdin.
        let agent = agent_with("cat", None);
        let receipt = agent.send(&request_for("hello handover"), None).unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        assert!(receipt
            .stdout
            .as_deref()
            .unwrap_or("")
            .contains("hello handover"));
    }

    #[test]
    fn substitutes_double_quoted_prompt_placeholder() {
        let agent = agent_with("printf %s \"{PROMPT}\"", None);
        let receipt = agent.send(&request_for("exact prompt text"), None).unwrap();
        assert!(
            receipt.ok,
            "{}: {}",
            receipt.detail,
            receipt.stderr.as_deref().unwrap_or("")
        );
        assert_eq!(receipt.stdout.as_deref().unwrap_or(""), "exact prompt text");
    }

    #[test]
    fn substitutes_bare_prompt_placeholder() {
        // A bare placeholder gets wrapped in quotes automatically.
        let agent = agent_with("printf %s {PROMPT}", None);
        let receipt = agent.send(&request_for("bare prompt"), None).unwrap();
        assert!(
            receipt.ok,
            "{}: {}",
            receipt.detail,
            receipt.stderr.as_deref().unwrap_or("")
        );
        assert_eq!(receipt.stdout.as_deref().unwrap_or(""), "bare prompt");
    }

    #[test]
    fn escaping_survives_tricky_prompt_text() {
        let agent = agent_with("printf %s \"{PROMPT}\"", None);
        let tricky = "it's a $tring with \"quotes\" and `backticks`";
        let receipt = agent.send(&request_for(tricky), None).unwrap();
        assert!(
            receipt.ok,
            "{}: {}",
            receipt.detail,
            receipt.stderr.as_deref().unwrap_or("")
        );
        assert_eq!(receipt.stdout.as_deref().unwrap_or(""), tricky);
    }

    #[test]
    fn substitutes_prompt_file_placeholder() {
        let agent = agent_with("wc -c < \"{PROMPT_FILE}\"", None);
        let receipt = agent.send(&request_for("hello\nworld"), None).unwrap();
        assert!(receipt.ok);
        assert!(receipt.stdout.as_deref().unwrap_or("").contains("11"));
    }

    #[test]
    fn prompt_file_placeholder_receives_prompt() {
        let agent = agent_with("cat \"{PROMPT_FILE}\"", None);
        let receipt = agent.send(&request_for("file prompt body"), None).unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        assert!(receipt
            .stdout
            .as_deref()
            .unwrap_or("")
            .contains("file prompt body"));
    }

    #[test]
    fn literal_prompt_placeholder_in_prompt_is_not_resubstituted() {
        // Regression (command injection): the old substitute() re-scanned
        // inserted prompt content with sequential str::replace calls, so a
        // prompt containing a literal `{PROMPT}` was substituted again —
        // pushing prompt text out of the protective quotes. The single-pass
        // scanner must never substitute inserted text.
        let marker = std::env::temp_dir()
            .join(format!("ho-inject-{}", uuid::Uuid::new_v4()))
            .display()
            .to_string();
        let attack = format!("p {{PROMPT}}; touch {marker}; #");
        let agent = agent_with("printf %s \"{PROMPT}\"", None);
        let receipt = agent.send(&request_for(&attack), None).unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        // The whole attack string must round-trip as inert prompt text…
        assert!(receipt.stdout.as_deref().unwrap_or("").contains("{PROMPT}"));
        // …and the smuggled command must NOT have executed.
        assert!(
            !std::path::Path::new(&marker).exists(),
            "injection executed: command was {}",
            receipt.detail
        );
    }

    #[test]
    fn literal_quoted_placeholders_in_prompt_stay_inert() {
        // Same rescan hole through every placeholder form, quoted and bare.
        let marker = std::env::temp_dir()
            .join(format!("ho-inject2-{}", uuid::Uuid::new_v4()))
            .display()
            .to_string();
        let forms = [
            format!("x \"{{PROMPT}}\"; touch {marker}; #"),
            format!("x '{{PROMPT}}'; touch {marker}; #"),
            format!("x {{PROMPT_FILE}}; touch {marker}; #"),
            format!("x {{SESSION}}; touch {marker}; #"),
        ];
        for attack in forms {
            let agent = agent_with("printf %s \"{PROMPT}\"", None);
            let receipt = agent.send(&request_for(&attack), None).unwrap();
            assert!(
                receipt.ok,
                "{}: {}",
                receipt.detail,
                receipt.stderr.as_deref().unwrap_or("")
            );
            assert!(
                !std::path::Path::new(&marker).exists(),
                "injection executed for {attack:?}"
            );
        }
    }

    #[test]
    fn honors_working_directory() {
        let dir = std::env::temp_dir().join(format!("ad-cwd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let agent = GenericCommandAgent::new(AgentConfig {
            id: "test".into(),
            name: "pwd".into(),
            kind: handover_config::AgentKind::Command,
            command: "pwd".into(),
            description: None,
            working_dir: Some(dir.to_string_lossy().to_string()),
            env: Default::default(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: None,
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let receipt = agent.send(&request_for("x"), None).unwrap();
        assert!(receipt.ok);
        assert!(receipt
            .stdout
            .as_deref()
            .unwrap_or("")
            .contains(dir.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn times_out_on_hung_command() {
        let agent = agent_with("sleep 5", Some(1));
        match agent.send(&request_for("x"), None) {
            Err(AgentError::Timeout { secs, .. }) => assert_eq!(secs, 1),
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[test]
    fn verbose_stdout_does_not_deadlock_or_timeout() {
        // Without concurrent drain, ~64KiB fills the pipe and the child blocks
        // forever while the parent waits for exit. Produce well over that.
        let agent = agent_with(
            r#"sh -c 'i=0; while [ "$i" -lt 8000 ]; do printf "line %s padding-padding-padding-padding\n" "$i"; i=$((i+1)); done'"#,
            Some(15),
        );
        let receipt = agent
            .send(&request_for("x"), None)
            .expect("verbose agent must complete without pipe deadlock");
        assert!(receipt.ok, "{}", receipt.detail);
        let out = receipt.stdout.as_deref().unwrap_or("");
        assert!(
            out.contains("truncated") || out.len() >= MAX_CAPTURED_OUTPUT_BYTES,
            "expected truncation note or full capped body; got {} bytes",
            out.len()
        );
        assert!(
            out.len() <= MAX_CAPTURED_OUTPUT_BYTES + 80,
            "stored stdout must be capped, got {}",
            out.len()
        );
    }

    /// Serializes tests that touch the process-global `HANDOVER_PROMPT_CACHE_DIR`
    /// env var (the override test sets it; the default-path test depends on it
    /// NOT being set).
    static PROMPT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn prompt_cache_dir_env_override_is_honored() {
        // `HANDOVER_PROMPT_CACHE_DIR` must redirect prompt temp files, so the
        // test suite / CI can pin the cache to a sandboxed location.
        let _guard = PROMPT_ENV_LOCK.lock().unwrap();

        let dir = std::env::temp_dir().join(format!("ho-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("HANDOVER_PROMPT_CACHE_DIR");
        std::env::set_var("HANDOVER_PROMPT_CACHE_DIR", &dir);

        let file = PromptTempFile::create("sandboxed prompt").expect("create prompt temp");
        let path = file.path().to_path_buf();
        assert!(
            path.starts_with(&dir),
            "prompt temp must live under HANDOVER_PROMPT_CACHE_DIR, got {}",
            path.display()
        );
        assert!(path.exists());
        drop(file);
        assert!(!path.exists(), "RAII Drop must remove the prompt temp file");

        match old {
            Some(v) => std::env::set_var("HANDOVER_PROMPT_CACHE_DIR", v),
            None => std::env::remove_var("HANDOVER_PROMPT_CACHE_DIR"),
        }
        // NOTE: never remove `dir` here — a parallel test may have already
        // created its own prompt file under it while the env var was set.
    }

    #[test]
    fn prompt_temp_file_is_private_mode_and_cleaned_up() {
        // Create a prompt file the same way send() does, verify mode + Drop cleanup.
        // (Directory snapshots across concurrent tests are flaky; this isolates one file.)
        // Holds the same lock as the env-override test: that test mutates the
        // global HANDOVER_PROMPT_CACHE_DIR this test implicitly depends on.
        let _guard = PROMPT_ENV_LOCK.lock().unwrap();
        let file = PromptTempFile::create("secret prompt body").expect("create prompt temp");
        let path = file.path().to_path_buf();
        assert!(
            path.exists(),
            "temp prompt should exist while guard is live"
        );
        assert!(
            path.to_string_lossy().contains("handover"),
            "prompt should live under an handover-private directory, not bare /tmp root: {}",
            path.display()
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "prompt temp file must be owner-read/write only"
            );
        }

        drop(file);
        assert!(!path.exists(), "RAII Drop must remove the prompt temp file");
    }

    #[test]
    fn prompt_file_placeholder_still_works_with_private_temp() {
        let agent = agent_with("cat \"{PROMPT_FILE}\"", None);
        let receipt = agent.send(&request_for("private temp body"), None).unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        assert!(receipt
            .stdout
            .as_deref()
            .unwrap_or("")
            .contains("private temp body"));
    }

    #[test]
    fn streams_output_chunks_in_order() {
        // The sink must receive stdout chunks as they are produced, in order,
        // before the process exits (that is what makes live output work).
        let agent = agent_with("printf 'first\\n'; sleep 0.05; printf 'second\\n'", None);
        let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink: OutputSink = {
            let chunks = Arc::clone(&chunks);
            Arc::new(move |_channel, chunk| chunks.lock().unwrap().push(chunk.to_string()))
        };
        let receipt = agent.send(&request_for("x"), Some(sink)).unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        let all = chunks.lock().unwrap().join("");
        assert!(all.contains("first"), "got: {all:?}");
        assert!(all.contains("second"), "got: {all:?}");
        assert!(
            all.find("first").unwrap() < all.find("second").unwrap(),
            "chunks must arrive in order: {all:?}"
        );
    }

    #[test]
    fn utf8_prefix_keeps_incomplete_trailing_sequence() {
        // '€' = E2 82 AC; the 4-byte emoji 🙂 = F0 9F 99 82.
        assert_eq!(utf8_complete_prefix_len(b""), 0);
        assert_eq!(utf8_complete_prefix_len(b"abc"), 3);
        assert_eq!(utf8_complete_prefix_len(b"ab\xE2"), 2); // lead byte only
        assert_eq!(utf8_complete_prefix_len(b"ab\xE2\x82"), 2); // lead + one continuation
        assert_eq!(utf8_complete_prefix_len(b"ab\xE2\x82\xAC"), 5); // complete
        assert_eq!(utf8_complete_prefix_len(b"x\xF0\x9F\x99"), 1); // 4-byte split mid-way
        assert_eq!(utf8_complete_prefix_len(b"x\xF0\x9F\x99\x82"), 5); // complete
    }

    /// A reader that yields at most 2 bytes per read, so a multi-byte UTF-8
    /// character is guaranteed to straddle chunk boundaries.
    struct SplitReader {
        data: Vec<u8>,
        pos: usize,
    }

    impl std::io::Read for SplitReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            let n = 2.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn drain_never_splits_multibyte_utf8() {
        let streamed = Arc::new(Mutex::new(String::new()));
        let sink: OutputSink = {
            let streamed = Arc::clone(&streamed);
            Arc::new(move |_channel, chunk| streamed.lock().unwrap().push_str(chunk))
        };
        let reader = SplitReader {
            data: b"a\xE2\x82\xACb\xF0\x9F\x99\x82".to_vec(), // "a€b🙂"
            pos: 0,
        };
        let handle = drain_capped(reader, Some(sink), OutputChannel::Stdout);
        let kept = handle.join().unwrap();
        assert_eq!(kept, "a€b🙂", "stored output must decode cleanly");
        assert_eq!(
            streamed.lock().unwrap().as_str(),
            "a€b🙂",
            "streamed chunks must not contain replacement characters"
        );
    }
}
