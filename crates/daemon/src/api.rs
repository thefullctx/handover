//! The local HTTP API. Binds to 127.0.0.1 only — nothing is ever exposed
//! to the network and nothing leaves the machine unless the user sends it.
//!
//! Mutating routes require a local bearer token (see `Config::api_token_path`).
//! `/health` stays open so process managers can probe liveness without secrets.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use handover_core::capture::Capture;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::SharedDaemon;

/// Hard ceiling on concurrently in-flight request threads.
///
/// One thread per request keeps a slow `/send` from ever blocking `/health`,
/// but it must stay bounded: `/agents` (unauthenticated) spawns `pgrep`/`ps`
/// and provider probes per call, and a misbehaving local client — or a
/// webpage firing loopback GETs — could otherwise grow threads without
/// limit. Overflow is answered with an immediate 503 instead of queuing.
const MAX_INFLIGHT_REQUESTS: usize = 16;

pub struct HttpHandle {
    pub port: u16,
}

/// Decrements the in-flight counter when a request thread finishes (or
/// panics — Drop runs during unwinding).
struct InflightGuard(Arc<AtomicUsize>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Starts serving the HTTP API on a background thread. If the port is taken
/// (another Handover instance) the caller decides whether that is fatal.
pub fn serve(
    daemon: SharedDaemon,
    port: u16,
    shutdown: Arc<AtomicBool>,
) -> Result<HttpHandle, String> {
    let server = Server::http(("127.0.0.1", port)).map_err(|e| {
        format!(
            "Could not bind to 127.0.0.1:{port} ({e}). Another Handover instance may already be running."
        )
    })?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);

    let inflight = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            // Bound concurrent request threads (see MAX_INFLIGHT_REQUESTS):
            // a burst beyond the cap is answered 503 immediately, never
            // queued. One thread per request means a slow `/send` (an agent
            // can run for minutes) never blocks `/health`, `/status` or a
            // capture.
            if inflight.fetch_add(1, Ordering::Relaxed) >= MAX_INFLIGHT_REQUESTS {
                inflight.fetch_sub(1, Ordering::Relaxed);
                respond(
                    request,
                    Err((
                        503,
                        format!(
                            "Handover is busy ({} requests in flight). Try again in a moment.",
                            MAX_INFLIGHT_REQUESTS
                        ),
                    )),
                );
                continue;
            }
            let request_daemon = daemon.clone();
            let request_shutdown = Arc::clone(&shutdown);
            let guard = InflightGuard(Arc::clone(&inflight));
            std::thread::spawn(move || {
                let _guard = guard;
                handle_request(request, request_daemon, request_shutdown);
            });
        }
    });

    Ok(HttpHandle { port: bound })
}

fn handle_request(mut request: Request, daemon: SharedDaemon, shutdown: Arc<AtomicBool>) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or("").to_string();

    let needs_auth = requires_auth(&method, &path);
    if needs_auth {
        let token = daemon.0.lock().unwrap().api_token.clone();
        if let Err(message) = authorize_request(&request, &token) {
            respond(request, Err((401, message)));
            return;
        }
    }

    // Cap request bodies: the API only carries prompts/captures (1 MiB cap
    // upstream) plus JSON framing. 8 MiB leaves ample headroom while bounding
    // a loopback OOM from a misbehaving local client.
    const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
    let mut body = Vec::new();
    {
        use std::io::Read;
        let mut reader = request.as_reader().take((MAX_BODY_BYTES + 1) as u64);
        if reader.read_to_end(&mut body).is_err() {
            respond(
                request,
                Err((400, "Could not read request body".to_string())),
            );
            return;
        }
    }
    if body.len() > MAX_BODY_BYTES {
        respond(request, Err((400, "Request body too large".to_string())));
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    let result = dispatch(&method, &path, &body, daemon, &shutdown);
    respond(request, result.map_err(|m| (400, m)));
}

/// Routes that can change state or trigger agent work require a bearer token.
fn requires_auth(method: &Method, path: &str) -> bool {
    match (method, path) {
        (Method::Get, "/health") => false,
        (Method::Get, "/status")
        | (Method::Get, "/actions")
        | (Method::Get, "/agents")
        | (Method::Get, "/preferences") => false,
        (Method::Post, _) => true,
        _ => true,
    }
}

fn authorize_request(request: &Request, expected_token: &str) -> Result<(), String> {
    if expected_token.is_empty() {
        return Err("Server has no API token configured.".to_string());
    }
    let expected_bearer = format!("Bearer {expected_token}");
    for header in request.headers() {
        // HeaderField → &str once (avoid as_str().as_str() double conversion).
        let name = header.field.as_str().to_string();
        if name.eq_ignore_ascii_case("Authorization") {
            let value = header.value.as_str().trim();
            if constant_time_eq(value.as_bytes(), expected_bearer.as_bytes()) {
                return Ok(());
            }
        }
        if name.eq_ignore_ascii_case("X-Handover-Token")
            && constant_time_eq(
                header.value.as_str().trim().as_bytes(),
                expected_token.as_bytes(),
            )
        {
            return Ok(());
        }
    }
    Err(
        "Unauthorized. Pass `Authorization: Bearer <token>` (token file is next to config.toml as `api_token`)."
            .to_string(),
    )
}

/// Constant-time byte comparison for secrets (prevents timing side-channels
/// between local processes probing the loopback token).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn dispatch(
    method: &Method,
    path: &str,
    body: &str,
    daemon: SharedDaemon,
    shutdown: &AtomicBool,
) -> Result<String, String> {
    let daemon = daemon.0;
    match (method, path) {
        (Method::Get, "/health") => Ok(json!({"ok": true}).to_string()),
        (Method::Get, "/status") => Ok(daemon.lock().unwrap().status().to_string()),
        (Method::Get, "/actions") => {
            let d = daemon.lock().unwrap();
            Ok(serde_json::to_string(&d.actions()).map_err(serde_err)?)
        }
        (Method::Get, "/agents") => {
            // Unauthenticated listing: redact full command lines so secrets
            // embedded in agent commands are not exposed on loopback GETs.
            // The desktop UI uses the in-process daemon and keeps full detail.
            //
            // Snapshot under the lock (cheap), probe off it: the process
            // check + provider-health probe must not block other requests.
            let snapshot = {
                let d = daemon.lock().unwrap();
                d.agents_status_snapshot()
            };
            let mut agents = crate::compute_agents_status(snapshot);
            for entry in &mut agents {
                if let Some(summary) = entry.meta.config_summary.as_mut() {
                    *summary = crate::redact_command_summary(summary);
                }
            }
            Ok(serde_json::to_string(&agents).map_err(serde_err)?)
        }
        (Method::Get, "/preferences") => {
            let d = daemon.lock().unwrap();
            Ok(serde_json::to_string(&d.config.preferences).map_err(serde_err)?)
        }
        (Method::Get, "/history") => {
            // Requires auth (falls through to `_ => true` in `requires_auth`):
            // history contains full prompts and agent replies.
            let d = daemon.lock().unwrap();
            Ok(serde_json::to_string(&d.history.recent()).map_err(serde_err)?)
        }
        (Method::Get, "/sessions") => {
            // Requires auth (falls through to `_ => true` in `requires_auth`):
            // session ids are opaque, but the listing reveals which agents the
            // user is running — keep it authenticated like /history.
            //
            // Snapshot under the lock (cheap), scan off it: glob walks and
            // cli-list subprocesses must not block other requests.
            let snapshot = {
                let d = daemon.lock().unwrap();
                d.live_sessions_snapshot()
            };
            Ok(
                serde_json::to_string(&crate::compute_live_sessions(&snapshot))
                    .map_err(serde_err)?,
            )
        }
        (Method::Post, "/sessions/pin") => {
            // `handover attach <agent> [session]`: pin a session so handoffs
            // resume into it (resolution order: explicit > pinned > freshest).
            // Without a session id the freshest live session is pinned — the
            // "run inside a session" flow. Explicit pins must be live (never
            // pin a phantom id). An optional `tty` records the approval
            // channel's target (the terminal you ran `attach` from).
            let params: Value = parse_body(body)?;
            let agent_id = required_str(&params, "agent_id")?;
            let session_id = params
                .get("session_id")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            let tty = params
                .get("tty")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            // Snapshot + scan off the lock: cli-list subprocesses and glob
            // walks must not block other requests while liveness is checked.
            let live = {
                let d = daemon.lock().unwrap();
                crate::compute_live_sessions(&d.live_sessions_snapshot())
            };
            let resolved = match session_id {
                Some(sid) => {
                    let is_live = live
                        .iter()
                        .any(|s| s.agent_id == agent_id && s.session_id == sid);
                    if !is_live {
                        return Err(format!(
                            "Session `{sid}` for `{agent_id}` is not live. Run `handover sessions` to list live sessions."
                        ));
                    }
                    sid
                }
                None => live
                    .iter()
                    .find(|s| s.agent_id == agent_id)
                    .map(|s| s.session_id.clone())
                    .ok_or_else(|| {
                        format!(
                            "No live session for `{agent_id}`. Start one (or pass an explicit session id)."
                        )
                    })?,
            };
            // Liveness was computed off the lock, so the session may have
            // ended in between. Fail soft: session resolution re-checks a
            // pinned session is live before every handoff, so a stale pin
            // never lands in a dead session.
            let mut d = daemon.lock().unwrap();
            d.pin_session(agent_id, &resolved)
                .map_err(|e| e.to_string())?;
            if let Some(t) = tty {
                if !t.trim().is_empty() {
                    d.set_approval_target(agent_id, &t)
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(json!({ "ok": true, "agent_id": agent_id, "session_id": resolved }).to_string())
        }
        (Method::Post, "/sessions/unpin") => {
            let params: Value = parse_body(body)?;
            let agent_id = required_str(&params, "agent_id")?;
            let mut d = daemon.lock().unwrap();
            d.unpin_session(agent_id).map_err(|e| e.to_string())?;
            Ok(json!({ "ok": true, "agent_id": agent_id }).to_string())
        }
        (Method::Post, "/sessions/approve") => {
            // Approve/deny a blocked session (Phase 6.5): the daemon re-checks
            // the session is still blocked, injects via the configured channel,
            // and verifies the transcript resumes (fail soft — never claims
            // success without verification).
            //
            // Two-phase: resolve under the lock (fast), execute without it
            // (slow — injection + 10s verify polling must not block other API
            // requests, the palette, or handoffs in flight).
            let params: Value = parse_body(body)?;
            let agent_id = required_str(&params, "agent_id")?;
            let session_id = required_str(&params, "session_id")?;
            let approve = params
                .get("approve")
                .and_then(|a| a.as_bool())
                .unwrap_or(true);
            let req = {
                // Plan under the lock (cheap config validation), complete
                // off it: the liveness re-check, still-blocked tail peek,
                // and command build read the filesystem.
                let plan = {
                    let d = daemon.lock().unwrap();
                    d.resolve_approval_plan(agent_id, session_id, approve)
                        .map_err(|e| e.to_string())?
                };
                crate::complete_approval(plan).map_err(|e| e.to_string())?
            };
            // Execute outside the lock: injection + verify polling (up to
            // 10s). Must not block /health, /status, palette opens, or
            // handoffs in flight.
            let result = crate::execute_approval(&req, crate::approval::APPROVAL_VERIFY_BUDGET)
                .map_err(|e| e.to_string())?;
            Ok(serde_json::to_string(&result).map_err(serde_err)?)
        }
        (Method::Post, "/preferences") => {
            let params: Value = parse_body(body)?;
            let action_id = required_str(&params, "action_id")?;
            let agent_id = required_str(&params, "agent_id")?;
            let mut d = daemon.lock().unwrap();
            d.set_preference(action_id, agent_id);
            Ok(json!({"ok": true}).to_string())
        }
        (Method::Post, "/render") => {
            let params: Value = parse_body(body)?;
            let action_id = required_str(&params, "action_id")?;
            let capture: Capture = capture_param(&params)?;
            let d = daemon.lock().unwrap();
            let prompt = d
                .render_prompt(action_id, capture)
                .map_err(|e| e.to_string())?;
            Ok(json!({"prompt": prompt}).to_string())
        }
        (Method::Post, "/send") => {
            let params: Value = parse_body(body)?;
            let action_id = required_str(&params, "action_id")?;
            let agent_id = params
                .get("agent_id")
                .and_then(|a| a.as_str())
                .map(|s| s.to_string());
            // Optional explicit session override (`handover send --session <id>`).
            let session_id = params
                .get("session_id")
                .and_then(|a| a.as_str())
                .map(|s| s.to_string());
            let capture = capture_param(&params)?;
            // Plan under the lock (fast): capture enrichment + config
            // resolution. Complete + execute with the lock released (slow) —
            // a long-running agent must not block other requests.
            let (agent, request) = {
                let plan = {
                    let d = daemon.lock().unwrap();
                    d.resolve_send_plan(
                        action_id,
                        agent_id.as_deref(),
                        session_id.as_deref(),
                        capture,
                    )
                    .map_err(|e| e.to_string())?
                };
                crate::complete_send(plan).map_err(|e| e.to_string())?
            };
            let outcome = crate::execute_handoff(agent, request, None, crate::next_handoff_id());
            // Record the completed handoff so /history and the tray menu see
            // it. `HandoffHistory` has its own lock — this is a quick push.
            daemon.lock().unwrap().history.record(outcome.clone());
            Ok(serde_json::to_string(&outcome).map_err(serde_err)?)
        }
        (Method::Post, "/quit") => {
            shutdown.store(true, Ordering::Relaxed);
            Ok(json!({"ok": true}).to_string())
        }
        _ => Err(format!("Not found: {method} {path}")),
    }
}

fn capture_param(params: &Value) -> Result<Capture, String> {
    let value = params
        .get("capture")
        .ok_or_else(|| "Missing `capture` in request body".to_string())?;
    serde_json::from_value(value.clone()).map_err(|e| format!("Invalid capture: {e}"))
}

fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing `{key}` in request body"))
}

fn parse_body(body: &str) -> Result<Value, String> {
    if body.trim().is_empty() {
        return Err("Empty request body".to_string());
    }
    serde_json::from_str(body).map_err(|e| format!("Invalid JSON body: {e}"))
}

fn serde_err(e: serde_json::Error) -> String {
    format!("Serialization error: {e}")
}

fn respond(request: Request, result: Result<String, (u16, String)>) {
    let (code, json) = match result {
        Ok(value) => (200, value),
        Err((code, message)) => {
            // Prefer 404 for unknown routes; keep 400 for bad client input.
            let code = if code == 400 && message.starts_with("Not found:") {
                404
            } else {
                code
            };
            (code, json!({"error": message}).to_string())
        }
    };
    let response = Response::from_string(json)
        .with_status_code(StatusCode(code))
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("valid header"),
        );
    let _ = request.respond(response);
}
