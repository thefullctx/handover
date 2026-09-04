//! Per-agent model-provider endpoint sniffing + cheap health probes.
//!
//! Handover never chooses or displays an agent's model — the agent's own CLI
//! does that. But a send dies uselessly when the provider behind that CLI is
//! unreachable (a local model server that isn't running, a dead proxy). This
//! module answers one question, fail-soft: *which endpoint will this agent
//! talk to on its next message?* The daemon probes it cheaply and can refuse
//! a doomed send in under a second with a real reason instead of hanging for
//! the full agent timeout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a probe result stays trusted (positive OR negative). Providers
/// flap; re-probing at most every few seconds keeps the palette responsive
/// without hammering endpoints.
const PROBE_TTL: Duration = Duration::from_secs(10);
/// A health probe must be near-instant to be worth doing inside a send.
const PROBE_TIMEOUT: Duration = Duration::from_millis(700);
/// How long a sniffed endpoint stays trusted before the agent's own config /
/// session state is read again. Users switch models mid-session (a Hermes
/// session can move from a local server to a cloud provider); the endpoint
/// must follow within a few status polls, not wait for a daemon restart.
const SNIFF_TTL: Duration = Duration::from_secs(30);

/// Where an agent's CLI finds its model provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEndpoint {
    /// `http(s)://host:port` — no path. For logging/messages only.
    pub base_url: String,
    /// Host as probed (e.g. `127.0.0.1`).
    pub host: String,
    /// Port as probed.
    pub port: u16,
}

impl ProviderEndpoint {
    fn parse(base_url: &str) -> Option<Self> {
        let (scheme, rest) = base_url.split_once("://")?;
        // Cloud endpoints rarely spell out a port (`https://api.example.com/v1`);
        // default by scheme so they probe like any other endpoint.
        let default_port = match scheme {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let authority = rest.split('/').next()?;
        // IPv6 brackets would need richer parsing; local providers are v4.
        let (host, port_str) = match authority.rsplit_once(':') {
            Some(pair) => pair,
            None => (authority, ""),
        };
        let port: u16 = if port_str.is_empty() {
            default_port
        } else {
            port_str.parse().ok()?
        };
        if host.is_empty() {
            return None;
        }
        Some(Self {
            base_url: base_url.to_string(),
            host: host.to_string(),
            port,
        })
    }
}

/// One cached probe verdict.
#[derive(Debug, Clone)]
struct ProbeVerdict {
    up: bool,
    at: Instant,
}

/// Sniffs + probes provider endpoints, memoized per agent id. Cheap enough
/// to call on every status refresh and before every send.
type EndpointCacheEntry = (Option<ProviderEndpoint>, Instant, Option<ProbeVerdict>);

/// Builds a fresh cache entry: sniff now, no verdict yet (the next
/// `endpoint_up` probes the — possibly changed — endpoint immediately).
fn fresh_entry(agent_id: &str, config_dir: &Path) -> EndpointCacheEntry {
    (sniff_endpoint(agent_id, config_dir), Instant::now(), None)
}

/// Re-sniffs when the cached endpoint is older than `ttl`, so a session
/// that switched providers mid-flight is followed without a daemon restart.
fn entry_with_fresh_sniff<'a>(
    agent_id: &str,
    config_dir: &Path,
    ttl: Duration,
    cache: &'a mut HashMap<String, EndpointCacheEntry>,
) -> &'a mut EndpointCacheEntry {
    let entry = cache
        .entry(agent_id.to_string())
        .or_insert_with(|| fresh_entry(agent_id, config_dir));
    if entry.1.elapsed() >= ttl {
        *entry = fresh_entry(agent_id, config_dir);
    }
    entry
}

/// Sniffs + probes provider endpoints, memoized per agent id. Cheap enough
/// to call on every status refresh and before every send.
pub struct ProviderHealth {
    cache: Mutex<HashMap<String, EndpointCacheEntry>>,
    config_dir: PathBuf,
    sniff_ttl: Duration,
}

impl Default for ProviderHealth {
    fn default() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            config_dir: dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
            sniff_ttl: SNIFF_TTL,
        }
    }
}

impl ProviderHealth {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test seam: point home-relative config paths somewhere else. The sniff
    /// TTL is zero here so tests can mutate configs and observe re-sniffs.
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn with_home(home: PathBuf) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            config_dir: home,
            sniff_ttl: Duration::ZERO,
        }
    }

    /// The endpoint `agent_id`'s CLI will most likely use next, sniffed from
    /// its own configuration. Unknown agents → `None` (never blocks).
    pub fn endpoint_for(&self, agent_id: &str) -> Option<ProviderEndpoint> {
        let mut cache = self.cache.lock().ok()?;
        entry_with_fresh_sniff(agent_id, &self.config_dir, self.sniff_ttl, &mut cache)
            .0
            .clone()
    }

    /// `true` when the endpoint answered recently. Re-probes after TTL.
    /// Errors fail OPEN (an unprovable endpoint must never block a send).
    pub fn endpoint_up(&self, agent_id: &str) -> bool {
        let mut cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return true,
        };
        let entry = entry_with_fresh_sniff(agent_id, &self.config_dir, self.sniff_ttl, &mut cache);
        let Some(ep) = &entry.0 else { return true };
        let fresh = matches!(&entry.2, Some(v) if v.at.elapsed() < PROBE_TTL);
        if !fresh {
            entry.2 = Some(ProbeVerdict {
                up: tcp_connectable(&ep.host, ep.port),
                at: Instant::now(),
            });
        }
        entry.2.as_ref().map(|v| v.up).unwrap_or(true)
    }

    /// Human-readable down-reason for the UI / refusal message, or `None`.
    pub fn down_reason(&self, agent_id: &str) -> Option<String> {
        if self.endpoint_up(agent_id) {
            return None;
        }
        self.endpoint_for(agent_id)
            .map(|ep| format!("provider down ({})", ep.base_url))
    }

    /// Send gate: `None` = clear to send; `Some(reason)` = refuse fast.
    /// Probes the endpoint THIS send will actually use — the target
    /// session's own provider for resumes, the ambient default for fresh
    /// sends — instead of the freshest session's provider. Probing the wrong
    /// one either waves through a doomed send or refuses a healthy one
    /// (e.g. TUI sessions on a cloud provider while the local default is
    /// down: fresh sends die while the freshest-session probe reads green).
    /// Unknown setups fail OPEN (attempt the send).
    pub fn send_readiness(&self, agent_id: &str, session_id: Option<&str>) -> Option<String> {
        match (agent_id, session_id) {
            ("hermes", Some(sid)) => {
                let ep = self
                    .hermes_session_endpoint(sid)
                    .or_else(|| self.hermes_ambient_endpoint());
                match ep {
                    Some(ep) if !tcp_connectable(&ep.host, ep.port) => {
                        Some(format!("session provider down ({})", ep.base_url))
                    }
                    _ => None,
                }
            }
            ("hermes", None) => match self.hermes_ambient_endpoint() {
                Some(ep) if !tcp_connectable(&ep.host, ep.port) => {
                    Some(format!("default provider down ({})", ep.base_url))
                }
                _ => None,
            },
            _ => self.down_reason(agent_id),
        }
    }

    /// The endpoint a resume into `session_id` will use: that session's own
    /// recorded provider. `None` when unknown (missing/locked DB, old schema,
    /// no row) — the caller falls back to the ambient default or fail-open.
    fn hermes_session_endpoint(&self, session_id: &str) -> Option<ProviderEndpoint> {
        let url = hermes_session_billing_url(
            &self.config_dir.join(".hermes/state.db"),
            session_id,
            PROBE_TIMEOUT.as_millis() as u64 * 4,
        )?;
        ProviderEndpoint::parse(&url)
    }

    /// The endpoint a fresh send will use: the agent's configured default.
    fn hermes_ambient_endpoint(&self) -> Option<ProviderEndpoint> {
        hermes_ambient_endpoint(&self.config_dir)
    }

    /// Drops all cached verdicts (config changed → force re-sniff).
    pub fn invalidate(&self) {
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }
}

/// Per-agent config locations. Each parser is tiny and fail-soft: any surprise
/// means "unknown", never an error shown to the user.
fn sniff_endpoint(agent_id: &str, home: &Path) -> Option<ProviderEndpoint> {
    match agent_id {
        "hermes" => {
            // 1) The FRESHEST live session's actual endpoint (Hermes records
            //    `billing_base_url` per session in state.db). A resumed
            //    session restores ITS provider — config defaults don't apply,
            //    so this is the truthful answer when a session exists.
            if let Some(ep) = sqlite_last_billing_base_url(
                &home.join(".hermes/state.db"),
                PROBE_TIMEOUT.as_millis() as u64 * 4,
            ) {
                return ProviderEndpoint::parse(&ep);
            }
            // 2) Fall back to the configured default (what fresh sends use).
            hermes_ambient_endpoint(home)
        }
        "codex" => {
            // TOML: `model_provider = "<id>"` then `[model_providers.<id>]`
            // with `base_url`. Fall back to the built-in OpenAI endpoint only
            // when explicitly configured — default codex needs no probe here.
            let text = read(home.join(".codex/config.toml"))?;
            let provider = find_toml_string(&text, "model_provider")?;
            let needle = "base_url".to_string();
            let section = format!("model_providers.{provider}");
            let url = toml_section_value(&text, &section, &needle)?;
            ProviderEndpoint::parse(url)
        }
        "opencode" => {
            // OpenCode reads config from ~/.config/opencode/opencode.json(.c).
            for name in ["opencode.jsonc", "opencode.json"] {
                if let Some(text) = read(home.join(".config/opencode").join(name)) {
                    if let Some(url) = find_json_string(&text, "baseURL")
                        .or_else(|| find_json_string(&text, "base_url"))
                    {
                        if let Some(ep) = ProviderEndpoint::parse(&url) {
                            return Some(ep);
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn read(p: PathBuf) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// The agent's configured default endpoint: what a fresh send will use.
/// Hermes: grep-style scan of the active `model:` block in config.yaml (no
/// YAML dependency). Shared by the status sniff (fallback) and the send
/// gate (the whole answer for fresh sends).
fn hermes_ambient_endpoint(home: &Path) -> Option<ProviderEndpoint> {
    let text = read(home.join(".hermes/config.yaml"))?;
    let mut in_model = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("model:") && !trimmed[6..].starts_with(['s', '_']) {
            in_model = true;
            continue;
        }
        if in_model {
            // Next top-level key ends the block.
            if !line.starts_with(' ') && !line.trim().is_empty() && line.contains(':') {
                break;
            }
            if let Some(u) = trimmed.trim().strip_prefix("base_url:") {
                if let Some(ep) = ProviderEndpoint::parse(u.trim().trim_matches('"')) {
                    return Some(ep);
                }
            }
        }
    }
    None
}

/// First `"key": "value"` occurrence in loose JSON/JSONC.
fn find_json_string(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let idx = text.find(&pat)?;
    let rest = &text[idx + pat.len()..];
    let colon = rest.find(':')?;
    let rest = rest[colon + 1..].trim_start();
    let quote = rest.strip_prefix('"')?;
    let end = quote.find('"')?;
    Some(quote[..end].to_string())
}

/// First bare `key = "value"` at file level (TOML, pre-sections).
fn find_toml_string(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            break; // entered sections — top-level keys are done
        }
        if let Some(rest) = t.strip_prefix(&format!("{key} =")) {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Value of `key` inside `[section]` (handles dotted section names and both
/// `=` and inline forms; stops at the next `[`).
fn toml_section_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let header = format!("[{section}]");
    let mut in_section = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_section = t == header;
            continue;
        }
        if in_section {
            if let Some(rest) = t.strip_prefix(key) {
                let rest = rest.trim_start();
                let rest = rest.strip_prefix('=')?.trim();
                return Some(rest.trim_matches('"'));
            }
        }
    }
    None
}

/// Can we open a TCP connection? That is all "provider is alive" means here —
/// no HTTP semantics, no auth, sub-second budget. Hostnames (`localhost`,
/// `*.local`, …) resolve via the system resolver; anything unresolvable or
/// malformed simply reads as "down" — never a panic.
///
/// DNS runs outside the connect timeout, so loopback names fast-path to
/// 127.0.0.1/::1 without touching the resolver (status polls must never stall
/// on DNS).
fn tcp_connectable(host: &str, port: u16) -> bool {
    use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
    // Fast-path loopback: no DNS, straight to connect_timeout.
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        let addrs = [
            SocketAddr::new("127.0.0.1".parse::<IpAddr>().unwrap(), port),
            SocketAddr::new("::1".parse::<IpAddr>().unwrap(), port),
        ];
        return addrs
            .into_iter()
            .any(|addr| std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok());
    }
    // connect_timeout needs a single SocketAddr; resolve first and try each
    // address (IPv4/IPv6). Fail open-free: errors mean "probe says down".
    match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs
            .into_iter()
            .try_fold(false, |_, addr| {
                if std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok() {
                    std::ops::ControlFlow::Break(true)
                } else {
                    std::ops::ControlFlow::Continue(false)
                }
            })
            .is_break(),
        Err(_) => false,
    }
}

/// Hard wall-clock cap on the whole sqlite3 spawn. Must exceed the busiest
/// in-flight busy timeout we pass (2800ms) so the DB's own fast-fail fires
/// first and the reason is distinguishable; the outer bound exists for the
/// pathological case where even `.timeout` can't save us (hung NFS mount,
/// wedged binary) — sniffing is a status read and must never stall it.
const SQLITE_SNIFF_BUDGET: Duration = Duration::from_millis(3_000);

/// Reads the most recently active session's `billing_base_url` from Hermes'
/// own state DB (read-only, busy-timeout bounded). `None` on any surprise:
/// missing file, locked DB, no such column (older versions), or empty value.
fn sqlite_last_billing_base_url(db_path: &PathBuf, busy_ms: u64) -> Option<String> {
    use std::process::Command;
    // No sqlite dependency: shell out to the sqlite3 CLI. Resolve portably
    // (Finder-launched apps inherit a restricted PATH): absolute candidates
    // first, then the bare name for the inherited PATH.
    let sqlite = resolve_sqlite3();
    let mut cmd = Command::new(sqlite);
    cmd.arg("-readonly")
        // `-cmd` and its value MUST be separate argv entries: one glued
        // argument like "-cmd .timeout 2800" is rejected as an unknown
        // option, the whole read fails, and the sniff silently falls back
        // to the (often stale) config default.
        .arg("-cmd")
        .arg(format!(".timeout {busy_ms}"))
        .arg(db_path)
        .arg(
            "SELECT billing_base_url FROM sessions WHERE \
             billing_base_url IS NOT NULL AND billing_base_url != '' \
             ORDER BY last_activity_at DESC LIMIT 1;",
        );
    // The daemon's bounded runner: piped+drained, killed at the wall-clock
    // budget. Replaces the previous `.stderr(Stdio::null()).output()` —
    // stderr is piped here but the caller reads stdout only, so the change
    // is unobservable.
    let out = crate::run_command_bounded(&mut cmd, SQLITE_SNIFF_BUDGET)?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !url.is_empty() {
        Some(url)
    } else {
        None
    }
}

/// Reads ONE session's `billing_base_url` by id (same bounds and fail-soft
/// contract as above). The id is allow-listed to session-id characters, so
/// the interpolated SQL cannot break out of its string literal.
fn hermes_session_billing_url(db_path: &PathBuf, session_id: &str, busy_ms: u64) -> Option<String> {
    use std::process::Command;
    if session_id.is_empty()
        || session_id.len() > 128
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }
    let sqlite = resolve_sqlite3();
    let mut cmd = Command::new(sqlite);
    cmd.arg("-readonly")
        .arg("-cmd")
        .arg(format!(".timeout {busy_ms}"))
        .arg(db_path)
        .arg(format!(
            "SELECT billing_base_url FROM sessions WHERE id='{session_id}' LIMIT 1;"
        ));
    let out = crate::run_command_bounded(&mut cmd, SQLITE_SNIFF_BUDGET)?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && !url.is_empty() {
        Some(url)
    } else {
        None
    }
}

/// Portable sqlite3 resolution: absolute system paths first (immune to
/// restricted PATH), then the bare name for the inherited PATH.
fn resolve_sqlite3() -> String {
    for candidate in [
        "/usr/bin/sqlite3",
        "/opt/homebrew/bin/sqlite3",
        "/usr/local/bin/sqlite3",
    ] {
        if std::path::Path::new(candidate).is_file() {
            return candidate.to_string();
        }
    }
    "sqlite3".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_base_urls_into_host_port() {
        let ep = ProviderEndpoint::parse("http://127.0.0.1:8001/v1").unwrap();
        assert_eq!(ep.host, "127.0.0.1");
        assert_eq!(ep.port, 8001);
        assert_eq!(ep.base_url, "http://127.0.0.1:8001/v1");
        assert!(ProviderEndpoint::parse("not a url").is_none());
        assert!(ProviderEndpoint::parse("ftp://host/only").is_none());
    }

    #[test]
    fn parses_portless_cloud_urls_with_scheme_default_ports() {
        // Regression: a cloud session endpoint (no :port) used to fail to
        // parse, silently falling back to the stale config default and
        // reporting "provider down" against the wrong server.
        let ep = ProviderEndpoint::parse("https://opencode.ai/zen/v1").unwrap();
        assert_eq!(ep.host, "opencode.ai");
        assert_eq!(ep.port, 443);
        let ep = ProviderEndpoint::parse("http://example.com/v1").unwrap();
        assert_eq!(ep.host, "example.com");
        assert_eq!(ep.port, 80);
        assert!(ProviderEndpoint::parse("https://host:99999/x").is_none());
    }

    /// A unique temp dir for a provider-health test. Must include randomness:
    /// a pid-only dir collides when the OS recycles a pid from an earlier
    /// (possibly killed, cleanup-skipped) run — `CREATE TABLE` then fails on
    /// the leftover db, which flaked under repeated workspace runs.
    fn prov_test_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ho-prov-{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hermes_config_yields_active_model_block_base_url() {
        let dir = prov_test_dir("cfg");
        let cfg = dir.join(".hermes");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join("config.yaml"),
            "model:\n  provider: local\n  base_url: http://127.0.0.1:8001/v1\n  default: m1\ndisplay:\n  skin: dos\n",
        )
        .unwrap();
        let h = ProviderHealth::with_home(dir.clone());
        let ep = h.endpoint_for("hermes").expect("endpoint sniffed");
        assert_eq!(ep.port, 8001);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_agent_has_no_opinion() {
        let h = ProviderHealth::with_home(std::env::temp_dir());
        assert_eq!(h.endpoint_for("definitely-not-real"), None);
        assert!(h.endpoint_up("definitely-not-real"), "fail open");
        assert_eq!(h.down_reason("definitely-not-real"), None);
    }

    #[test]
    fn endpoint_follows_config_changes_without_invalidate() {
        // with_home() sets sniff_ttl = 0, so every call re-sniffs: a session
        // switching providers mid-flight must move the probe target.
        let dir = prov_test_dir("resniff");
        let cfg = dir.join(".hermes");
        std::fs::create_dir_all(&cfg).unwrap();
        let path = cfg.join("config.yaml");
        std::fs::write(
            &path,
            "model:\n  base_url: http://127.0.0.1:8001/v1\ndisplay:\n  skin: dos\n",
        )
        .unwrap();
        let h = ProviderHealth::with_home(dir.clone());
        assert_eq!(h.endpoint_for("hermes").unwrap().port, 8001);
        std::fs::write(
            &path,
            "model:\n  base_url: http://127.0.0.1:8010/v1\ndisplay:\n  skin: dos\n",
        )
        .unwrap();
        assert_eq!(
            h.endpoint_for("hermes").expect("re-sniffed").port,
            8010,
            "stale cached endpoint must be refreshed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sqlite_reader_reads_freshest_billing_url_with_busy_timeout() {
        // Regression: `-cmd` and its value were passed as ONE argv entry,
        // which sqlite3 rejects ("unknown option") — the read silently
        // failed on every machine and the sniff always fell back to the
        // stale config default. This exercises the real spawn path.
        use std::process::Command;
        let Ok(which) = Command::new("which").arg("sqlite3").output() else {
            return; // no sqlite3 in this environment: nothing to verify
        };
        if !which.status.success() {
            return;
        }
        let dir = prov_test_dir("sqlite");
        let db = dir.join("state.db");
        let setup = Command::new("sqlite3")
            .arg(&db)
            .arg(
                "DROP TABLE IF EXISTS sessions;\
                 CREATE TABLE sessions (billing_base_url TEXT, last_activity_at REAL);\
                 INSERT INTO sessions VALUES ('http://127.0.0.1:8001/v1', 100.0);\
                 INSERT INTO sessions VALUES ('', 200.0);\
                 INSERT INTO sessions VALUES ('https://opencode.ai/zen/v1', 300.0);",
            )
            .output()
            .unwrap();
        assert!(setup.status.success(), "failed to seed test db");
        assert_eq!(
            sqlite_last_billing_base_url(&db, 100),
            Some("https://opencode.ai/zen/v1".to_string()),
            "freshest non-empty billing_base_url must win"
        );
        assert_eq!(
            sqlite_last_billing_base_url(&dir.join("missing.db"), 100),
            None,
            "missing db fails soft"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tcp_probe_handles_hostnames_and_garbage_without_panic() {
        // `localhost` (a hostname) must resolve and probe, not panic —
        // regression guard for the SocketAddr-only unwrap. Nothing needs to
        // be listening: the assertion is only "no panic + a bool".
        let _ = tcp_connectable("localhost", 9); // discard port: closed or refused
                                                 // Unresolvable garbage must read as down.
        assert!(!tcp_connectable("definitely.not.a.real.host.invalid", 8001));
        // A listening loopback socket must read as up.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_connectable("127.0.0.1", port));
    }

    // -- send_readiness: gate on the endpoint the send will use -------------

    static GATE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn gate_dir(prefix: &str) -> PathBuf {
        let n = GATE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ho-prov-{prefix}-{}-{n}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join(".hermes")).unwrap();
        dir
    }

    /// A reserved loopback port that refuses connections.
    ///
    /// The socket is bound but never put into the listening state, so any
    /// connect gets ECONNREFUSED while the reservation is held. A bare
    /// `bind → drop → port` was racy under parallel test execution: another
    /// test could re-bind the freed port between the drop and the probe,
    /// which made `send_gate_fresh_probes_ambient_default` flake under
    /// `cargo test --workspace`.
    struct ClosedPort {
        port: u16,
        // Keeps the bound (non-listening) socket alive so no other test or
        // process can bind the port while this guard is in scope.
        #[cfg(unix)]
        _reservation: std::os::unix::io::OwnedFd,
    }

    impl ClosedPort {
        fn port(&self) -> u16 {
            self.port
        }
    }

    /// Reserves a loopback port with a bound-but-not-listening TCP socket.
    /// Per `bind(2)` semantics, incoming connects to a socket that was never
    /// `listen(2)`-ed are refused (RST) — exactly what these tests need.
    #[cfg(unix)]
    fn closed_port() -> ClosedPort {
        use std::os::fd::{FromRawFd, OwnedFd};
        // SAFETY: a fresh kernel socket fd; ownership moves to OwnedFd below,
        // which closes it on drop.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(
            fd >= 0,
            "socket() failed: {}",
            std::io::Error::last_os_error()
        );
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        // s_addr is stored in network byte order; `.to_be()` writes
        // 127.0.0.1 as the bytes 7F 00 00 01 on any endianness.
        addr.sin_addr.s_addr = libc::INADDR_LOOPBACK.to_be();
        addr.sin_port = 0; // kernel picks an ephemeral port
                           // SAFETY: `addr` is a fully initialized sockaddr_in and `fd` a valid
                           // socket; the kernel copies the address — no aliasing.
        let rc = unsafe {
            libc::bind(
                fd,
                &addr as *const libc::sockaddr_in as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        assert_eq!(rc, 0, "bind() failed: {}", std::io::Error::last_os_error());
        // Read back the port the kernel assigned.
        let mut len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        // SAFETY: getsockname writes one sockaddr_in (length checked by the
        // kernel against `len`).
        let rc = unsafe {
            libc::getsockname(
                fd,
                &mut addr as *mut libc::sockaddr_in as *mut libc::sockaddr,
                &mut len,
            )
        };
        assert_eq!(
            rc,
            0,
            "getsockname() failed: {}",
            std::io::Error::last_os_error()
        );
        // sin_port is stored in network byte order; from_be recovers the
        // numeric port regardless of host endianness.
        let port = u16::from_be(addr.sin_port);
        // SAFETY: `fd` was created by this function and is not used anywhere
        // else — OwnedFd now owns and closes it.
        let reservation = unsafe { OwnedFd::from_raw_fd(fd) };
        ClosedPort {
            port,
            _reservation: reservation,
        }
    }

    #[cfg(not(unix))]
    fn closed_port() -> ClosedPort {
        // No raw-socket reservation off Unix; bind+drop is racy under
        // parallel tests, acceptable outside the tested mac/linux matrix.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        ClosedPort {
            port: l.local_addr().unwrap().port(),
        }
    }

    fn write_ambient(dir: &std::path::Path, port: u16) {
        std::fs::write(
            dir.join(".hermes/config.yaml"),
            format!("model:\n  base_url: http://127.0.0.1:{port}/v1\ndisplay:\n  skin: dos\n"),
        )
        .unwrap();
    }

    #[test]
    fn send_gate_fresh_probes_ambient_default() {
        // Dead ambient default → refuse fast with a fresh-specific reason.
        // The guard keeps the refusing port reserved for the whole test — a
        // freed port could be re-bound by a concurrent test before the probe.
        let dir = gate_dir("fresh-down");
        let closed = closed_port();
        write_ambient(&dir, closed.port());
        let h = ProviderHealth::with_home(dir.clone());
        let reason = h.send_readiness("hermes", None).expect("must refuse");
        assert!(reason.contains("default provider down"), "{reason}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_gate_fresh_passes_when_ambient_up() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let dir = gate_dir("fresh-up");
        write_ambient(&dir, port);
        let h = ProviderHealth::with_home(dir.clone());
        assert_eq!(h.send_readiness("hermes", None), None);
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn send_gate_resume_probes_target_session_not_freshest() {
        use std::process::Command;
        let Ok(which) = Command::new("which").arg("sqlite3").output() else {
            return; // no sqlite3 in this environment: nothing to verify
        };
        if !which.status.success() {
            return;
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let up = listener.local_addr().unwrap().port();
        // Guard stays in scope until the assertions below are done so the
        // refusing port cannot be re-bound by a concurrent test.
        let _down_guard = closed_port();
        let down = _down_guard.port();
        // Ambient points at the DOWN port: fresh sends refuse, but a resume
        // into the session on the UP endpoint must pass (and vice versa) —
        // the gate follows the send, not the freshest session.
        let dir = gate_dir("resume-target");
        write_ambient(&dir, down);
        let db = dir.join(".hermes/state.db");
        let setup = Command::new("sqlite3")
            .arg(&db)
            .arg(format!(
                "CREATE TABLE sessions (id TEXT, billing_base_url TEXT, last_activity_at REAL);\
                 INSERT INTO sessions VALUES ('20260101_000000_aaaaaa', 'http://127.0.0.1:{up}/v1', 100.0);\
                 INSERT INTO sessions VALUES ('20260101_000001_bbbbbb', 'http://127.0.0.1:{down}/v1', 200.0);"
            ))
            .output()
            .unwrap();
        assert!(setup.status.success(), "failed to seed test db");
        let h = ProviderHealth::with_home(dir.clone());
        assert_eq!(
            h.send_readiness("hermes", Some("20260101_000000_aaaaaa")),
            None,
            "resume into the UP session passes despite dead ambient default"
        );
        let reason = h
            .send_readiness("hermes", Some("20260101_000001_bbbbbb"))
            .expect("must refuse");
        assert!(reason.contains("session provider down"), "{reason}");
        let reason = h.send_readiness("hermes", None).expect("must refuse");
        assert!(reason.contains("default provider down"), "{reason}");
        // Unknown agents fail open; unknown sessions assume ambient (down here).
        assert_eq!(h.send_readiness("definitely-not-real", None), None);
        assert!(
            h.send_readiness("hermes", Some("no-such-session"))
                .is_some(),
            "unknown session falls back to the (down) ambient default"
        );
        drop(listener);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
