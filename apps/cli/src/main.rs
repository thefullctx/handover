//! Handover CLI.
//!
//! The CLI is a thin client of the running Handover daemon — it contains
//! no application logic of its own. Everything is orchestrated by the daemon
//! over the local HTTP API.
//!
//! ```
//! handover status
//! handover agents
//! handover actions
//! handover send "Fix this error"
//! handover send ./screenshot.png
//! cat error.log | handover
//! ```

use std::io::{IsTerminal, Read};
use std::time::Duration;

use serde_json::{json, Value};

const DEFAULT_PORT: u16 = 47444;

const HELP: &str = "\
Handover CLI — hand anything to an AI agent in seconds.

Usage:
  handover status                 Show daemon status and agent availability
  handover agents                 List configured agents
  handover actions                List available actions
  handover sessions               List live agent sessions (freshest first, with activity)
  handover send [text|file...]    Send a capture to an agent
       --agent <id>                Agent to use (default: your preference, else default agent)
       --action <id>               Action to use (default: ask)
       --session <id>              Resume into this live session (overrides pinned/freshest)
       --print-prompt              Print the prompt that would be sent, without sending
  handover attach <agent> [id]    Pin a session so handoffs resume into it
       --unpin                     Clear the pin (freshest-first resumes)
       --tty <dev>                 Record the approval target (default: auto-detect your tty)
  handover approve <agent> [id]   Approve a blocked session (opt-in, per agent)
       --deny                      Deny instead of approve
  handover help                   Show this help

Examples:
  handover send \"Fix this error\"
  handover send ./screenshot.png --action explain --agent qwen-local
  handover send --session 20260812_130220_dbf5cf \"keep going on that\"
  handover sessions
  handover attach hermes          # run inside a Hermes session to pin it
  handover approve codex          # approve the blocked codex session
  cat error.log | handover
  handover status

Environment:
  HANDOVER_PORT           daemon port (default 47444)
  HANDOVER_HOST           daemon host (default 127.0.0.1)
  HANDOVER_TOKEN          override local API bearer token (default: read from config dir)
  HANDOVER_TIMEOUT_SECS   override HTTP wait for /send and /render (seconds, min 30).
                           Default: daemon max agent timeout + 60s grace (at least 180s)
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => {}
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let Some(first) = args.first() else {
        // `cat error.log | handover` (no subcommand, non-TTY stdin) → send.
        // Interactive bare `handover` still prints help.
        if !std::io::stdin().is_terminal() {
            return cmd_send(&[]);
        }
        print!("{HELP}");
        return Ok(());
    };
    match first.as_str() {
        "status" => cmd_status(),
        "agents" => cmd_agents(),
        "actions" => cmd_actions(),
        "sessions" => cmd_sessions(),
        "attach" => cmd_attach(&args[1..]),
        "approve" => cmd_approve(&args[1..]),
        "send" => cmd_send(&args[1..]),
        "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(())
        }
        other if other.starts_with('-') => {
            print!("{HELP}");
            Err(format!("Unknown option `{other}`"))
        }
        // `handover "some text"` — treat remaining args as send input.
        _ => cmd_send(args),
    }
}

fn cmd_status() -> Result<(), String> {
    let status = request("GET", "/status", None)?;
    let agents = request("GET", "/agents", None)?;

    println!(
        "Handover daemon: {}",
        if status["daemon"].as_bool().unwrap_or(false) {
            "running"
        } else {
            "unknown"
        }
    );
    println!("Version:  {}", status["version"].as_str().unwrap_or("?"));
    println!("Platform: {}", status["platform"].as_str().unwrap_or("?"));
    println!("Port:     {}", status["port"].as_u64().unwrap_or(0));
    println!("Actions:  {}", status["actions"].as_u64().unwrap_or(0));
    println!(
        "Config:   {}",
        status["config_path"].as_str().unwrap_or("?")
    );
    println!();

    if let Some(list) = agents.as_array() {
        if list.is_empty() {
            println!(
                "Agents: none configured (edit {})",
                handover_config::Config::config_path_display()
            );
        } else {
            println!("Agents:");
            for item in list {
                let name = item["meta"]["name"].as_str().unwrap_or("?");
                let id = item["meta"]["id"].as_str().unwrap_or("?");
                let available = item["status"]["available"].as_bool().unwrap_or(false);
                let detail = item["status"]["detail"].as_str().unwrap_or("");
                let state = if available {
                    "available".to_string()
                } else {
                    format!("not available — {detail}")
                };
                println!("  {id:<20} {name:<24} {state}");
            }
        }
    }
    Ok(())
}

fn cmd_agents() -> Result<(), String> {
    let agents = request("GET", "/agents", None)?;
    let list = agents.as_array().ok_or("Unexpected agents response")?;
    if list.is_empty() {
        println!(
            "No agents configured. Edit {} and restart.",
            handover_config::Config::config_path_display()
        );
        return Ok(());
    }
    for item in list {
        let id = item["meta"]["id"].as_str().unwrap_or("?");
        let name = item["meta"]["name"].as_str().unwrap_or("?");
        let kind = item["meta"]["kind"].as_str().unwrap_or("?");
        let available = item["status"]["available"].as_bool().unwrap_or(false);
        let summary = item["meta"]["config_summary"].as_str().unwrap_or("");
        println!(
            "{id}\t{name}\t[{kind}]\t{}: {}{}",
            if available {
                "available"
            } else {
                "unavailable"
            },
            item["status"]["detail"].as_str().unwrap_or(""),
            if summary.is_empty() {
                String::new()
            } else {
                format!("\t{summary}")
            }
        );
    }
    Ok(())
}

fn cmd_actions() -> Result<(), String> {
    let actions = request("GET", "/actions", None)?;
    let list = actions.as_array().ok_or("Unexpected actions response")?;
    for item in list {
        println!(
            "{}: {} — {}",
            item["id"].as_str().unwrap_or("?"),
            item["name"].as_str().unwrap_or("?"),
            item["description"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

fn cmd_sessions() -> Result<(), String> {
    let sessions = request("GET", "/sessions", None)?;
    let list = sessions.as_array().ok_or("Unexpected sessions response")?;
    if list.is_empty() {
        println!("No live sessions. Start an agent session, then open Handover.");
        return Ok(());
    }
    println!("Live sessions (freshest first):");
    for item in list {
        let agent = item["agent_id"].as_str().unwrap_or("?");
        let sid = item["session_id"].as_str().unwrap_or("?");
        let updated = item["updated_at"].as_str().unwrap_or("");
        let size = item["size_bytes"].as_u64();
        let size_str = size.map(|n| format!(" · {n} B")).unwrap_or_default();
        // Blocked (at an approval prompt) is the most important state — it
        // wins the status column; working/idle derive from mtime.
        let blocked = item["blocked"].as_bool().unwrap_or(false);
        let state = if blocked {
            "blocked".to_string()
        } else {
            item["activity"].as_str().unwrap_or("?").to_string()
        };
        println!(
            "  {agent:<12} {state:<9} {:<10} {sid}{size_str}",
            session_time(updated)
        );
    }
    Ok(())
}

/// Pins a session so handoffs resume into it: `handover attach <agent> [id]`.
/// With no id, the freshest live session for the agent is pinned — the "run
/// inside a session" flow. `--unpin` clears the pin (freshest-first resumes).
/// When run inside the agent's terminal, the current tty is recorded as the
/// approval target so Approve/Deny can inject into that session.
fn cmd_attach(args: &[String]) -> Result<(), String> {
    let mut unpin = false;
    let mut tty_override: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--unpin" => unpin = true,
            "--tty" => {
                i += 1;
                tty_override = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--tty requires a device path (e.g. /dev/ttys002)")?,
                );
            }
            other if other.starts_with("--") => return Err(format!("Unknown option `{other}`")),
            value => positional.push(value.to_string()),
        }
        i += 1;
    }
    let agent_id = positional
        .first()
        .cloned()
        .ok_or("Usage: handover attach <agent-id> [session-id] [--tty <dev>]")?;
    if unpin {
        request(
            "POST",
            "/sessions/unpin",
            Some(json!({ "agent_id": agent_id })),
        )?;
        println!("✓ Unpinned {agent_id} — session awareness resumes freshest-first");
        return Ok(());
    }
    let mut body = json!({ "agent_id": agent_id });
    if let Some(sid) = positional.get(1) {
        body["session_id"] = json!(sid);
    }
    // Record the tty this command runs on (the agent's terminal), so the
    // approval layer knows where to inject. Explicit --tty wins.
    let tty = tty_override.or_else(current_tty);
    if let Some(t) = &tty {
        body["tty"] = json!(t);
    }
    let res = request("POST", "/sessions/pin", Some(body))?;
    let sid = res["session_id"].as_str().unwrap_or("?");
    println!("✓ Pinned {agent_id} → session {sid}");
    if let Some(t) = tty {
        println!("  Approval target recorded: {t}");
    }
    println!("  Handoffs will resume into it while it stays live.");
    Ok(())
}

/// The tty device this process is attached to (e.g. `/dev/ttys002`), via the
/// `tty` utility. Returns `None` when stdin is not a terminal.
fn current_tty() -> Option<String> {
    let output = std::process::Command::new("sh")
        .args(["-c", "tty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if tty.is_empty() || tty == "not a tty" {
        None
    } else {
        Some(tty)
    }
}

/// Approves or denies a blocked session (opt-in, per agent): `handover
/// approve <agent> [session] [--deny]`. The daemon re-checks the session is
/// still blocked, injects via the configured channel, and verifies the
/// transcript resumes — success is only claimed when verified.
fn cmd_approve(args: &[String]) -> Result<(), String> {
    let mut deny = false;
    let mut positional: Vec<String> = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--deny" => deny = true,
            other if other.starts_with("--") => return Err(format!("Unknown option `{other}`")),
            value => positional.push(value.to_string()),
        }
    }
    let agent_id = positional
        .first()
        .cloned()
        .ok_or("Usage: handover approve <agent-id> [session-id] [--deny]")?;
    // No explicit session → the freshest live session for the agent.
    let session_id = positional.get(1).cloned().unwrap_or_else(|| {
        let sessions = request("GET", "/sessions", None).unwrap_or(Value::Null);
        sessions
            .as_array()
            .and_then(|list| list.iter().find(|s| s["agent_id"] == agent_id))
            .and_then(|s| s["session_id"].as_str().map(|x| x.to_string()))
            .unwrap_or_default()
    });
    if session_id.is_empty() {
        return Err(format!(
            "No live session for `{agent_id}`. Run `handover sessions` to list them."
        ));
    }
    let res = request(
        "POST",
        "/sessions/approve",
        Some(json!({
            "agent_id": agent_id,
            "session_id": session_id,
            "approve": !deny,
        })),
    )?;
    let verified = res["verified"].as_bool().unwrap_or(false);
    let message = res["message"].as_str().unwrap_or("");
    if verified {
        println!("✓ {message}");
        Ok(())
    } else {
        // Fail soft: the decision was sent but not confirmed — surface it.
        println!("⚠ {message}");
        Ok(())
    }
}

/// "2:14pm"-style local time for a session listing (parse-fail-safe).
fn session_time(iso: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return "?".to_string();
    };
    let local = parsed.with_timezone(&chrono::Local);
    local.format("%-I:%M%p").to_string()
}

fn cmd_send(args: &[String]) -> Result<(), String> {
    let mut agent_id: Option<String> = None;
    let mut action_id = "ask".to_string();
    let mut session_id: Option<String> = None;
    let mut print_prompt = false;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--agent" => {
                i += 1;
                agent_id = Some(args.get(i).cloned().ok_or("--agent requires an agent id")?);
            }
            "--action" => {
                i += 1;
                action_id = args
                    .get(i)
                    .cloned()
                    .ok_or("--action requires an action id")?;
            }
            "--session" => {
                i += 1;
                session_id = Some(
                    args.get(i)
                        .cloned()
                        .ok_or("--session requires a session id")?,
                );
            }
            "--print-prompt" => print_prompt = true,
            other if other.starts_with("--") => return Err(format!("Unknown option `{other}`")),
            value => positional.push(value.to_string()),
        }
        i += 1;
    }

    let capture = build_capture(&positional)?;
    let body = json!({
        "action_id": action_id,
        "agent_id": agent_id,
        "session_id": session_id,
        "capture": capture,
    });

    if print_prompt {
        let rendered = request(
            "POST",
            "/render",
            Some(json!({
                "action_id": action_id,
                "capture": capture,
            })),
        )?;
        print!("{}", rendered["prompt"].as_str().unwrap_or(""));
        if !rendered["prompt"].as_str().unwrap_or("").ends_with('\n') {
            println!();
        }
        return Ok(());
    }

    let result = request("POST", "/send", Some(body))?;
    let ok = result["ok"].as_bool().unwrap_or(false);
    let agent = result["agent_name"].as_str().unwrap_or("?");

    if ok {
        let ms = result["receipt"]["duration_ms"].as_u64().unwrap_or(0);
        println!("✓ Handed off to {agent} ({action_id}) in {ms} ms");
        if let Some(sid) = result["session_id"].as_str() {
            if !sid.is_empty() {
                println!("  session: {sid}");
            }
        }
        if let Some(detail) = result["receipt"]["detail"].as_str() {
            if !detail.is_empty() {
                println!("  {detail}");
            }
        }
        Ok(())
    } else {
        let error = result["error"].as_str().unwrap_or("Unknown error");
        Err(format!("✗ {error}"))
    }
}

/// Builds the capture JSON sent to the daemon. The daemon normalizes it and
/// (for file captures) reads + validates the file against privacy exclusions.
fn build_capture(positional: &[String]) -> Result<Value, String> {
    let stdin_is_terminal = std::io::stdin().is_terminal();

    let (content, source_kind) = if !positional.is_empty() {
        let value = positional.join(" ");
        let path = std::path::Path::new(&value);
        // Multiple existing files are ambiguous (one capture carries one
        // path): fail loud instead of silently sending "a.log b.log" as text.
        if positional.len() > 1 && !path.is_file() {
            let files: Vec<&str> = positional
                .iter()
                .filter(|a| std::path::Path::new(a).is_file())
                .map(String::as_str)
                .collect();
            if files.len() >= 2 {
                return Err(format!(
                    "Multiple files ({}). Send one at a time — e.g. `handover send {}`.",
                    files.join(", "),
                    files[0]
                ));
            }
        }
        if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic"
            ) {
                (
                    json!({"type": "image", "path": value, "text": null}),
                    "file",
                )
            } else {
                (json!({"type": "file", "path": value, "text": null}), "file")
            }
        } else {
            (
                json!({"type": "text", "text": value, "path": null}),
                "manual",
            )
        }
    } else if !stdin_is_terminal {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|e| format!("Could not read stdin: {e}"))?;
        if buffer.trim().is_empty() {
            return Err("No input received from stdin.".to_string());
        }
        (
            json!({"type": "text", "text": buffer, "path": null}),
            "terminal",
        )
    } else {
        return Err(
            "No input. Pass text, a file path, or pipe stdin — e.g. `cat error.log | handover send`."
                .to_string(),
        );
    };

    // Useful, cheap context (Phase 7 light): working directory + git branch.
    let mut metadata = json!({});
    if let Ok(cwd) = std::env::current_dir() {
        metadata["working_directory"] = json!(cwd.to_string_lossy());
        if let Some(branch) = git_branch(&cwd) {
            metadata["git_branch"] = json!(branch);
        }
    }

    Ok(json!({
        "source": {"type": source_kind, "application": "handover-cli"},
        "content": content,
        "metadata": metadata,
    }))
}

fn git_branch(dir: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("branch")
        .arg("--show-current")
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

fn daemon_url() -> String {
    let host = std::env::var("HANDOVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("HANDOVER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    format!("http://{host}:{port}")
}

/// Resolves the local API bearer token: env override, else the token file
/// next to the platform config (created by the daemon/desktop on first run).
fn api_token() -> Option<String> {
    if let Ok(token) = std::env::var("HANDOVER_TOKEN") {
        let trimmed = token.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    handover_config::Config::load_api_token()
}

/// How long the CLI will wait for the daemon on long operations (`/send`).
/// Prefer the daemon's max agent timeout (+ grace) so a 300s agent is not
/// cut off by a shorter HTTP client timeout.
fn http_timeout_for(path: &str) -> Duration {
    // Quick GETs and light POSTs don't need multi-minute budgets.
    let long_op = path == "/send" || path == "/render";
    if !long_op {
        return Duration::from_secs(30);
    }
    if let Ok(secs) = std::env::var("HANDOVER_TIMEOUT_SECS") {
        if let Ok(n) = secs.parse::<u64>() {
            return Duration::from_secs(n.max(30));
        }
    }
    // Probe /status for max_agent_timeout_secs when possible (short timeout).
    let from_daemon = fetch_max_agent_timeout_secs().unwrap_or(120);
    // Agent may run for max_agent_timeout_secs; add grace for IPC/serialization.
    Duration::from_secs(from_daemon.saturating_add(60).max(180))
}

fn fetch_max_agent_timeout_secs() -> Option<u64> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();
    let url = format!("{}/status", daemon_url());
    let mut req = agent.get(&url);
    if let Some(t) = api_token() {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = req.call().ok()?;
    let v: Value = resp.into_json().ok()?;
    v.get("max_agent_timeout_secs")
        .and_then(|x| x.as_u64())
        .or_else(|| v.get("default_agent_timeout_secs").and_then(|x| x.as_u64()))
}

fn request(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let timeout = http_timeout_for(path);
    let agent = ureq::AgentBuilder::new().timeout(timeout).build();
    let url = format!("{}{}", daemon_url(), path);
    let token = api_token();

    let response = match method {
        "GET" => {
            let mut req = agent.get(&url);
            if let Some(ref t) = token {
                req = req.set("Authorization", &format!("Bearer {t}"));
            }
            req.call()
        }
        "POST" => {
            let mut req = agent.post(&url);
            if let Some(ref t) = token {
                req = req.set("Authorization", &format!("Bearer {t}"));
            }
            req.send_json(body.unwrap_or(Value::Null))
        }
        _ => return Err(format!("Unsupported method {method}")),
    };

    match response {
        Ok(r) => r.into_json().map_err(|e| format!("Bad response from daemon: {e}")),
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let message = body
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error");
            if code == 401 {
                Err(format!(
                    "{message} (HTTP {code})\nTip: ensure the desktop app or daemon has created {} (or set HANDOVER_TOKEN).",
                    handover_config::Config::api_token_path().display()
                ))
            } else {
                Err(format!("{message} (HTTP {code})"))
            }
        }
        Err(ureq::Error::Transport(t)) if t.to_string().to_lowercase().contains("timed out")
            || format!("{t}").to_lowercase().contains("timeout") =>
        {
            Err(format!(
                "Timed out waiting for the daemon after {}s while calling {path}.\n\
The agent may still be running on the daemon. Increase HANDOVER_TIMEOUT_SECS \
or lower the agent's timeout_secs in config.",
                timeout.as_secs()
            ))
        }
        Err(e) => Err(format!(
            "Cannot reach the Handover daemon at {url} ({e}).\nStart Handover (the menu-bar app) or run `handover-daemon` in a terminal."
        )),
    }
}
