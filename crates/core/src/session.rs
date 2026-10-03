//! Live-session discovery for session-aware handoff.
//!
//! Session-aware handoff reduces to: "find the most recently touched session
//! file for the agent that is running; the filename IS the session id;
//! resume by id." This module implements the pure, testable core of that:
//! glob expansion over well-known agent session directories, session-id
//! extraction from filenames, staleness cutoff, and mtime-derived activity
//! state (working / idle).
//!
//! PRIVACY: discovery reads filenames + mtimes only — never transcript
//! contents. The one structural exception is cli-list agents (Hermes, whose
//! sessions live in a SQLite DB with no filesystem glob); for those we parse
//! the read-only output of an agent CLI command (`hermes sessions list`),
//! which returns session ids and titles, not message contents. Running the
//! command belongs to the daemon layer; this module parses the captured text.
//!
//! HARDENING: glob expansion never follows symlinks at the wildcard or leaf
//! level (mirrors the O_NOFOLLOW discipline in the enrichment pipeline), so a
//! planted symlink or session file cannot redirect a handoff. Literal
//! intermediate segments use the normal `is_dir` check (trusted parents per
//! the documented threat model, and system roots like macOS `/tmp`).

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::exclusions::matches_glob;

/// How recently a transcript must have been touched to count as "working".
pub const WORKING_WINDOW: Duration = Duration::seconds(30);
/// Default staleness cutoff: sessions older than this are dropped.
pub const DEFAULT_STALENESS: Duration = Duration::days(7);

/// What an agent is doing right now, derived purely from transcript mtime.
///
/// "Working" means the transcript was touched within [`WORKING_WINDOW`];
/// "Idle" means it is quiet but still within the staleness cutoff. An
/// approval pause is *quiet* and indistinguishable from idle by mtime —
/// that distinction belongs to the approval layer (Phase 6.5), not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityState {
    Working,
    Idle,
}

/// A live (non-stale) agent session discovered on disk or via a CLI listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSession {
    pub agent_id: String,
    pub session_id: String,
    pub updated_at: DateTime<Utc>,
    /// Transcript path, when the agent persists sessions as files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// Transcript size in bytes (stat-only, privacy-compliant size proxy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    pub activity: ActivityState,
    /// Whether the session is parked at an approval prompt. Only set for
    /// agents with a `permission_marker` configured — the ONE documented
    /// privacy exception (a tail-peek of the last few KB; see the approval
    /// module). `None` = the agent is not approval-aware (never peeked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    /// The marker line itself (the prompt the agent is asking) when the
    /// session is blocked — shown on the approval card so the user sees
    /// WHAT is being asked, not just that something is. Same single,
    /// opt-in tail-peek as `blocked`; `None` when not blocked. Never
    /// persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_detail: Option<String>,
}

/// How a session-aware agent exposes its sessions to discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionDiscovery {
    /// Expand a glob (relative to the home dir, `~` prefix optional) into
    /// session transcript files. Filename -> session id via
    /// [`session_id_from_filename`].
    Glob { pattern: String },
    /// Run an agent CLI command whose stdout lists sessions (Hermes).
    /// The daemon executes the command; [`parse_cli_list_output`] parses it.
    CliList { command: Vec<String> },
}

/// A catalog entry describing how to discover live sessions for one agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSpec {
    pub agent_id: String,
    pub discovery: SessionDiscovery,
}

impl SessionSpec {
    pub fn glob(agent_id: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            discovery: SessionDiscovery::Glob {
                pattern: pattern.into(),
            },
        }
    }

    pub fn cli_list(agent_id: impl Into<String>, command: Vec<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            discovery: SessionDiscovery::CliList { command },
        }
    }
}

/// Activity state from a transcript mtime.
pub fn activity_state(updated_at: DateTime<Utc>, now: DateTime<Utc>) -> ActivityState {
    if now - updated_at <= WORKING_WINDOW {
        ActivityState::Working
    } else {
        ActivityState::Idle
    }
}

/// True when a session is older than the staleness cutoff.
pub fn is_stale(updated_at: DateTime<Utc>, now: DateTime<Utc>, staleness: Duration) -> bool {
    now - updated_at > staleness
}

/// Agent ids that need a rule other than "the filename stem IS the id" in
/// [`session_id_from_filename`].
///
/// Exported so the config crate can assert every one of these has an
/// `ADAPTER_COMPAT` declaration: an arm added to the per-agent match without
/// declaring which agent version it was verified against is the silent-drift
/// case, and this is what makes CI fail on it.
pub const SESSION_ID_RULE_AGENTS: &[&str] = &["claude", "droid", "codex", "omp"];

/// Extracts the session id from a session transcript filename.
///
/// Per-agent rules (see the Phase 0 facts table in ARCHITECTURE.md, "Per-agent
/// session facts", and the verified versions in `config::ADAPTER_COMPAT`). This
/// is a maintenance point for agent CLI drift — keep all three in sync: an arm
/// added here must be listed in [`SESSION_ID_RULE_AGENTS`] and declared in
/// `ADAPTER_COMPAT`.
pub fn session_id_from_filename(agent_id: &str, path: &Path) -> Option<String> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    match agent_id {
        // claude / droid: <uuid>.jsonl -> uuid
        "claude" | "droid" => Some(stem.to_string()),
        // codex: rollout-<ts>-<uuid>.jsonl -> trailing uuid
        // timestamp is 7 dash-separated parts after "rollout-"; the uuid
        // is the last 5 parts (8-4-4-4-12).
        "codex" => {
            let rest = stem.strip_prefix("rollout-")?;
            let parts: Vec<&str> = rest.split('-').collect();
            if parts.len() < 5 {
                return None;
            }
            Some(parts[parts.len() - 5..].join("-"))
        }
        // omp: <timestamp>_<sessionId>.jsonl -> part after the last '_'
        "omp" => stem.rsplit_once('_').map(|(_, id)| id.to_string()),
        _ => Some(stem.to_string()),
    }
}

/// Expands a simple glob pattern into matching regular files.
///
/// Supports `*` and `?` within a path segment (via the shared glob matcher)
/// and a leading `~` for the home directory. Relative patterns are resolved
/// against `home`. Only regular files are returned; symlinks are never
/// followed (see module docs). A wildcard segment does not match dotfiles,
/// matching shell/find semantics.
fn expand_glob(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let trimmed = pattern.trim();
    let (base, rest) = if let Some(rest) = trimmed.strip_prefix("~/") {
        (home.to_path_buf(), rest)
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        (PathBuf::from("/"), rest)
    } else {
        (home.to_path_buf(), trimmed)
    };
    let segments: Vec<&str> = rest.split('/').collect();

    let mut out = Vec::new();
    walk_glob(&base, &segments, &mut out);
    out
}

fn walk_glob(dir: &Path, segments: &[&str], out: &mut Vec<PathBuf>) {
    let Some(seg) = segments.first() else {
        return;
    };
    let rest = &segments[1..];
    let is_last = rest.is_empty();

    if seg.contains('*') || seg.contains('?') {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            // DirEntry::file_type does not follow symlinks.
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || !matches_glob(seg, &name) {
                continue;
            }
            if is_last {
                if ft.is_file() {
                    out.push(entry.path());
                }
            } else if ft.is_dir() {
                walk_glob(&entry.path(), rest, out);
            }
        }
    } else {
        let child = dir.join(seg);
        if is_last {
            if let Ok(meta) = std::fs::symlink_metadata(&child) {
                if meta.file_type().is_file() {
                    out.push(child);
                }
            }
        } else if child.is_dir() {
            // Literal intermediate segments are trusted parents (fixed names
            // from the pattern, e.g. `.claude/projects`, plus system roots
            // like macOS `/tmp -> /private/tmp`). Symlink planting is stopped
            // at the wildcard level above (entries) and the leaf level
            // (symlink_metadata): a planted `evil -> /etc` can only enter via
            // a `*` segment, which never follows symlinks.
            walk_glob(&child, rest, out);
        }
    }
}

/// Scans all glob-based session specs and returns every non-stale session,
/// freshest first. `staleness` drops sessions older than the cutoff.
///
/// CliList specs are ignored here — the daemon runs those commands and feeds
/// the output through [`live_sessions_from_cli_rows`].
pub fn scan_live_sessions(
    specs: &[SessionSpec],
    now: DateTime<Utc>,
    staleness: Duration,
) -> Vec<LiveSession> {
    scan_live_sessions_home(specs, now, staleness, &home_dir())
}

/// [`scan_live_sessions`] with an explicit home (tests use a temp dir).
pub fn scan_live_sessions_home(
    specs: &[SessionSpec],
    now: DateTime<Utc>,
    staleness: Duration,
    home: &Path,
) -> Vec<LiveSession> {
    let mut sessions = Vec::new();
    for spec in specs {
        let SessionDiscovery::Glob { pattern } = &spec.discovery else {
            continue;
        };
        for path in expand_glob(pattern, home) {
            let Some(session_id) = session_id_from_filename(&spec.agent_id, &path) else {
                continue;
            };
            // Reject symlinks without following them: a planted symlink swapped
            // in after glob expansion must not redirect the stat/read.
            let Ok(sym) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !sym.file_type().is_file() {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let Ok(updated) = meta.modified() else {
                continue;
            };
            let updated_at: DateTime<Utc> = updated.into();
            if is_stale(updated_at, now, staleness) {
                continue;
            }
            sessions.push(LiveSession {
                agent_id: spec.agent_id.clone(),
                session_id,
                updated_at,
                path: Some(path),
                size_bytes: Some(meta.len()),
                activity: activity_state(updated_at, now),
                blocked: None,
                blocked_detail: None,
            });
        }
    }
    sort_freshest_first(&mut sessions);
    sessions
}

/// Keeps only the freshest session per agent (first occurrence after sorting
/// freshest-first). The daemon uses this for the default target, and the
/// full list (before this filter) for the ambiguity picker.
pub fn freshest_per_agent(sessions: Vec<LiveSession>) -> Vec<LiveSession> {
    let mut seen = std::collections::HashSet::new();
    sessions
        .into_iter()
        .filter(|s| seen.insert(s.agent_id.clone()))
        .collect()
}

fn sort_freshest_first(sessions: &mut [LiveSession]) {
    sessions.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.agent_id.cmp(&b.agent_id))
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
}

/// One row parsed from a cli-list agent's session listing (e.g. Hermes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliListRow {
    pub session_id: String,
    pub title: Option<String>,
    /// Only set when the listing carries an absolute timestamp; otherwise
    /// the caller treats the row as live (see [`live_sessions_from_cli_rows`]).
    pub updated_at: Option<DateTime<Utc>>,
    /// The listing's own recency column (Hermes prints "Last Active" as
    /// `8m ago` / `2d ago` / `now`), in seconds. This is the HONEST
    /// freshness signal — far better than the id-encoded creation time.
    pub last_active: Option<i64>,
}

/// Parses the stdout of a cli-list agent's session listing.
///
/// Handles `hermes sessions list`, which prints one row per session with the
/// session id as the final whitespace-delimited token
/// (`YYYYMMDD_HHMMSS_<hex>`); separator lines and headers are skipped.
/// Message contents are never part of this listing.
pub fn parse_cli_list_output(output: &str) -> Vec<CliListRow> {
    let mut rows = Vec::new();
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(token) = tokens.last() else {
            continue;
        };
        if !is_hermes_session_id(token) {
            continue;
        }
        // Title is everything before the trailing tokens; keep it short.
        let title = line
            .rsplit_once(char::is_whitespace)
            .map(|(t, _)| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string());
        rows.push(CliListRow {
            session_id: token.to_string(),
            title,
            updated_at: None,
            last_active: parse_last_active(&tokens),
        });
    }
    // Freshest first: rows with a recency column lead (smaller = more
    // recent); rows without one sort last, keeping listing order.
    rows.sort_by_key(|r| r.last_active.unwrap_or(i64::MAX));
    rows
}

/// Parses the listing's recency column (the token(s) right before the session
/// id): `… 8m ago <id>`, `… now <id>`. Anchored at the END of the line so a
/// title containing "ago" can never be mistaken for recency. Returns seconds;
/// `None` when the listing has no recency column.
fn parse_last_active(tokens: &[&str]) -> Option<i64> {
    let n = tokens.len();
    if n < 2 {
        return None;
    }
    // "… now <id>" (also covers "just now" — the extra word is ignored).
    if tokens[n - 2] == "now" {
        return Some(0);
    }
    // "… 8m ago <id>" — unit token then "ago".
    if n >= 3 && tokens[n - 2] == "ago" {
        let unit = tokens[n - 3];
        let (num, suffix) = unit.split_at(unit.len().saturating_sub(1));
        let mult = match suffix {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            "d" => 86_400,
            "w" => 604_800,
            _ => return None,
        };
        return num.parse::<i64>().ok().map(|v| v * mult);
    }
    None
}

fn is_hermes_session_id(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.len() != 8 + 1 + 6 + 1 + 6 {
        return false;
    }
    let ok =
        |range: std::ops::Range<usize>, pred: fn(u8) -> bool| bytes[range].iter().all(|&b| pred(b));
    ok(0..8, |b| b.is_ascii_digit())
        && bytes[8] == b'_'
        && ok(9..15, |b| b.is_ascii_digit())
        && bytes[15] == b'_'
        && ok(16..22, |b| b.is_ascii_hexdigit())
}

/// Converts parsed cli-list rows into [`LiveSession`]s.
///
/// Rows without an absolute timestamp are treated as live with `updated_at =
/// now` (the listing itself is the freshness signal) and activity Idle —
/// mtime granularity does not exist for cli-list agents.
pub fn live_sessions_from_cli_rows(
    agent_id: &str,
    rows: Vec<CliListRow>,
    now: DateTime<Utc>,
    staleness: Duration,
) -> Vec<LiveSession> {
    let mut sessions = Vec::new();
    for row in rows {
        // Freshness order: the listing's own recency column wins (the honest
        // signal), then an absolute timestamp, then the id-encoded creation
        // time (fallback — still far better than treating every row as
        // equally fresh, which made the freshest session sort last).
        let updated_at = row
            .updated_at
            .or_else(|| row.last_active.map(|secs| now - Duration::seconds(secs)))
            .or_else(|| hermes_id_timestamp(&row.session_id))
            .unwrap_or(now);
        if is_stale(updated_at, now, staleness) {
            continue;
        }
        sessions.push(LiveSession {
            agent_id: agent_id.to_string(),
            session_id: row.session_id,
            updated_at,
            path: None,
            size_bytes: None,
            // Derive activity from the recency signal like glob sessions — a
            // "now" row is Working, an "8m ago" row is Idle.
            activity: activity_state(updated_at, now),
            blocked: None,
            blocked_detail: None,
        });
    }
    sort_freshest_first(&mut sessions);
    sessions
}

/// Parses a Hermes session id's creation timestamp (`20260814_030109_abc123`
/// — local time) as UTC. `None` for ids that do not match the shape.
pub fn hermes_id_timestamp(id: &str) -> Option<DateTime<Utc>> {
    use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
    let parts: Vec<&str> = id.split('_').collect();
    if parts.len() != 3 {
        return None;
    }
    let date = NaiveDate::parse_from_str(parts[0], "%Y%m%d").ok()?;
    let time = chrono::NaiveTime::parse_from_str(parts[1], "%H%M%S").ok()?;
    let naive = NaiveDateTime::new(date, time);
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&Utc))
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("handover-session-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- session_id_from_filename -----------------------------------------

    #[test]
    fn session_id_rule_agents_stay_listed_and_correct() {
        // `SESSION_ID_RULE_AGENTS` exists so the config crate can assert every
        // id handled by the per-agent match has an ADAPTER_COMPAT declaration.
        // If it drifts from the arms below, that guard silently stops
        // guarding — so assert the listed ids still extract correctly.
        let rollout = Path::new(
            "/x/sessions/2026/08/14/rollout-2026-08-14T00-27-18-019ffd73-95b5-7240-b6bb-a39b1952730b.jsonl",
        );
        // codex is the one rule that genuinely differs from the default
        // (filename stem) — it must never drop out of the list.
        assert!(
            SESSION_ID_RULE_AGENTS.contains(&"codex"),
            "codex has a custom session-id rule and must stay listed"
        );
        assert_eq!(
            session_id_from_filename("codex", rollout).as_deref(),
            Some("019ffd73-95b5-7240-b6bb-a39b1952730b")
        );

        let uuid_path = Path::new("/x/projects/proj1/3b2c1a4e-8f6d-4b1e-9c2a-5d4f3e2b1a0c.jsonl");
        // claude / droid take the stem as-is; codex only ever sees `rollout-`
        // files, so a bare uuid is correctly rejected rather than guessed at.
        for agent_id in ["claude", "droid"] {
            assert!(
                SESSION_ID_RULE_AGENTS.contains(&agent_id),
                "{agent_id}: has an explicit session-id arm and must stay listed"
            );
            assert_eq!(
                session_id_from_filename(agent_id, uuid_path).as_deref(),
                Some("3b2c1a4e-8f6d-4b1e-9c2a-5d4f3e2b1a0c"),
                "{agent_id}: must keep taking the filename stem as the id"
            );
        }
        assert_eq!(
            session_id_from_filename("codex", uuid_path),
            None,
            "codex must not invent an id from a non-rollout filename"
        );
        for agent_id in SESSION_ID_RULE_AGENTS {
            assert!(!agent_id.is_empty(), "listed agent ids must be non-empty");
        }

        // An agent NOT listed falls through to the default (stem) rule.
        assert_eq!(
            session_id_from_filename("opencode", rollout).as_deref(),
            Some("rollout-2026-08-14T00-27-18-019ffd73-95b5-7240-b6bb-a39b1952730b"),
            "an unlisted agent uses the default stem rule"
        );
    }

    #[test]
    fn extracts_claude_and_droid_uuids() {
        let p = Path::new("/x/projects/proj1/3b2c1a4e-8f6d-4b1e-9c2a-5d4f3e2b1a0c.jsonl");
        assert_eq!(
            session_id_from_filename("claude", p).as_deref(),
            Some("3b2c1a4e-8f6d-4b1e-9c2a-5d4f3e2b1a0c")
        );
        assert_eq!(
            session_id_from_filename("droid", p).as_deref(),
            Some("3b2c1a4e-8f6d-4b1e-9c2a-5d4f3e2b1a0c")
        );
    }

    #[test]
    fn extracts_codex_uuid_after_rollout_timestamp() {
        let p = Path::new(
            "/x/sessions/2026/08/14/rollout-2026-08-14T00-27-18-019ffd73-95b5-7240-b6bb-a39b1952730b.jsonl",
        );
        assert_eq!(
            session_id_from_filename("codex", p).as_deref(),
            Some("019ffd73-95b5-7240-b6bb-a39b1952730b")
        );
    }

    #[test]
    fn extracts_omp_id_after_timestamp() {
        let p = Path::new(
            "/x/sessions/--private-tmp--/2026-08-12T16-44-19-970Z_019ff6dc-4a82-7000-8f74-e0cf04a0b6fe.jsonl",
        );
        assert_eq!(
            session_id_from_filename("omp", p).as_deref(),
            Some("019ff6dc-4a82-7000-8f74-e0cf04a0b6fe")
        );
    }

    #[test]
    fn rejects_non_jsonl_filenames() {
        assert_eq!(
            session_id_from_filename("claude", Path::new("/x/session.md")),
            None
        );
        assert_eq!(
            session_id_from_filename("codex", Path::new("/x/rollout-short.jsonl")),
            None
        );
    }

    // -- glob expansion ---------------------------------------------------

    #[test]
    fn expands_simple_glob_tree() {
        let home = temp_home();
        let proj = home.join(".codex/sessions/2026/08/14");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("rollout-2026-08-14T00-27-18-019ffd73-95b5-7240-b6bb-a39b1952730b.jsonl"),
            "",
        )
        .unwrap();
        std::fs::write(
            proj.join("rollout-2026-08-14T01-00-00-deadbeef-0000-4000-8000-000000000000.jsonl"),
            "",
        )
        .unwrap();
        // A stray non-session file and a nested dir must not match.
        std::fs::write(proj.join("notes.md"), "").unwrap();
        std::fs::create_dir_all(home.join(".codex/sessions/2026/08/14/sub")).unwrap();

        let found = expand_glob(".codex/sessions/*/*/*/*.jsonl", &home);
        assert_eq!(found.len(), 2);
        for f in &found {
            assert!(f.extension().is_some_and(|e| e == "jsonl"));
        }
    }

    #[test]
    fn supports_tilde_prefix_and_missing_dirs() {
        let home = temp_home();
        std::fs::create_dir_all(home.join(".claude/projects/proj1")).unwrap();
        std::fs::write(
            home.join(".claude/projects/proj1/3b2c1a4e-8f6d-4b1e-9c2a-5d4f3e2b1a0c.jsonl"),
            "",
        )
        .unwrap();

        assert_eq!(expand_glob("~/.claude/projects/*/*.jsonl", &home).len(), 1);
        // Missing top-level dir -> empty, no panic.
        assert!(expand_glob("~/.nonexistent/*/*.jsonl", &home).is_empty());
        // No `*` and nothing at the path -> empty.
        assert!(expand_glob("~/.claude/projects/proj1", &home).is_empty());
    }

    #[test]
    fn never_follows_symlinks() {
        let home = temp_home();
        let real = temp_home();
        std::fs::write(real.join("planted.jsonl"), "").unwrap();
        let proj = home.join(".claude/projects/proj1");
        std::fs::create_dir_all(&proj).unwrap();
        std::os::unix::fs::symlink(real.join("planted.jsonl"), proj.join("victim.jsonl")).unwrap();

        assert!(expand_glob("~/.claude/projects/*/*.jsonl", &home).is_empty());
    }

    #[test]
    fn wildcard_segments_skip_dotfiles() {
        let home = temp_home();
        std::fs::create_dir_all(home.join("sessions/.hidden")).unwrap();
        std::fs::write(home.join("sessions/.hidden/session.jsonl"), "").unwrap();
        std::fs::create_dir_all(home.join("sessions/visible")).unwrap();
        std::fs::write(home.join("sessions/visible/session.jsonl"), "").unwrap();

        let found = expand_glob("sessions/*/*.jsonl", &home);
        assert_eq!(found.len(), 1);
        assert!(found[0].to_string_lossy().contains("visible"));
    }

    // -- scan_live_sessions -----------------------------------------------

    #[test]
    fn scan_returns_all_sessions_freshest_first_with_activity() {
        let home = temp_home();
        let day = home.join("sessions/2026/08/14");
        std::fs::create_dir_all(&day).unwrap();
        let old =
            day.join("rollout-2026-08-14T00-00-00-aaaaaaaa-0000-4000-8000-000000000000.jsonl");
        let fresh =
            day.join("rollout-2026-08-14T00-30-00-bbbbbbbb-0000-4000-8000-000000000000.jsonl");
        std::fs::write(&old, "").unwrap();
        std::fs::write(&fresh, "").unwrap();

        // Give the files distinct mtimes (fresh is newest).
        let now = Utc::now();
        let old_ts: DateTime<Utc> = now - Duration::minutes(25);
        let fresh_ts: DateTime<Utc> = now - Duration::seconds(5);
        set_mtime(&old, old_ts);
        set_mtime(&fresh, fresh_ts);

        let specs = vec![SessionSpec::glob("codex", "sessions/*/*/*/*.jsonl")];
        let sessions = scan_live_sessions_home(&specs, now, DEFAULT_STALENESS, &home);

        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0].session_id,
            "bbbbbbbb-0000-4000-8000-000000000000"
        );
        assert_eq!(sessions[0].activity, ActivityState::Working);
        assert_eq!(sessions[1].activity, ActivityState::Idle);
        assert!(sessions[0].path.is_some());
        assert!(sessions[0].size_bytes.is_some());
    }

    #[test]
    fn scan_drops_stale_sessions() {
        let home = temp_home();
        let day = home.join("sessions/2026/08/01");
        std::fs::create_dir_all(&day).unwrap();
        let stale =
            day.join("rollout-2026-08-01T00-00-00-cccccccc-0000-4000-8000-000000000000.jsonl");
        std::fs::write(&stale, "").unwrap();
        set_mtime(&stale, Utc::now() - Duration::days(8));

        let specs = vec![SessionSpec::glob("codex", "sessions/*/*/*/*.jsonl")];
        let sessions = scan_live_sessions_home(&specs, Utc::now(), DEFAULT_STALENESS, &home);
        assert!(sessions.is_empty());
    }

    #[test]
    fn scan_ignores_cli_list_specs_and_empty_dirs() {
        let home = temp_home();
        let specs = vec![
            SessionSpec::cli_list(
                "hermes",
                vec!["hermes".into(), "sessions".into(), "list".into()],
            ),
            SessionSpec::glob("codex", "sessions/*/*/*/*.jsonl"),
        ];
        let sessions = scan_live_sessions_home(&specs, Utc::now(), DEFAULT_STALENESS, &home);
        assert!(sessions.is_empty());
    }

    #[test]
    fn freshest_per_agent_keeps_one_per_agent() {
        let now = Utc::now();
        let mk = |agent: &str, id: &str, age_secs: i64| LiveSession {
            agent_id: agent.to_string(),
            session_id: id.to_string(),
            updated_at: now - Duration::seconds(age_secs),
            path: None,
            size_bytes: None,
            activity: ActivityState::Idle,
            blocked: None,
            blocked_detail: None,
        };
        let all = vec![
            mk("hermes", "newest", 1),
            mk("hermes", "oldest", 3600),
            mk("codex", "only", 10),
        ];
        let picked = freshest_per_agent(all);
        assert_eq!(picked.len(), 2);
        let hermes = picked.iter().find(|s| s.agent_id == "hermes").unwrap();
        assert_eq!(hermes.session_id, "newest");
    }

    // -- cli-list parsing (Hermes) ----------------------------------------

    #[test]
    fn parses_hermes_sessions_list_output() {
        let output = "\
──────────────────────────────────────────────────────────────────────────────
Investigate this issue and   neo                yesterday     20260812_130220_dbf5cf
List three planets #2        handover           8m ago        20260814_165718_668694
Friendly greeting            neo                yesterday     20260812_034825_a2a3e8
";
        let rows = parse_cli_list_output(output);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].session_id, "20260814_165718_668694");
        assert_eq!(rows[1].session_id, "20260812_130220_dbf5cf");
        assert_eq!(rows[2].session_id, "20260812_034825_a2a3e8");
        assert!(rows[1].title.as_deref().unwrap().contains("Investigate"));
        assert_eq!(rows[1].updated_at, None);
        // The listing's own "Last Active" column is parsed when present:
        // "8m ago" → 480s; no recency column ("yesterday") → None.
        assert_eq!(rows[0].last_active, Some(480));
        assert_eq!(rows[1].last_active, None);
    }

    #[test]
    fn parses_last_active_units() {
        let mk = |tail: &str| {
            let line = format!("Title here {tail} 20260812_130220_dbf5cf");
            let tokens: Vec<&str> = line.split_whitespace().collect();
            parse_last_active(&tokens)
        };
        assert_eq!(mk("8m ago"), Some(480));
        assert_eq!(mk("2d ago"), Some(2 * 86_400));
        assert_eq!(mk("45s ago"), Some(45));
        assert_eq!(mk("1h ago"), Some(3600));
        assert_eq!(mk("now"), Some(0));
        assert_eq!(mk("just now"), Some(0));
        // No recency column, or a title word that merely ends in "ago".
        assert_eq!(mk("yesterday"), None);
        assert_eq!(mk("Chicago"), None);
        // A title ending in "ago" before the id must not be misread.
        let tokens: Vec<&str> = "Shipped ago 20260812_130220_dbf5cf"
            .split_whitespace()
            .collect();
        assert_eq!(parse_last_active(&tokens), None);
    }

    #[test]
    fn parses_empty_and_garbage_output() {
        assert!(parse_cli_list_output("").is_empty());
        assert!(parse_cli_list_output("──────────────────────────────").is_empty());
        assert!(parse_cli_list_output("no sessions yet\n").is_empty());
    }

    #[test]
    fn cli_rows_become_live_sessions_with_idle_activity() {
        let now = Utc::now();
        // Use a session id whose encoded timestamp is within the staleness
        // window (constructed from `now` so the test never rots as dates pass).
        let ts = now - chrono::Duration::hours(2);
        let sid = format!("{}_{}_abc123", ts.format("%Y%m%d"), ts.format("%H%M%S"));
        let rows = vec![CliListRow {
            session_id: sid.clone(),
            title: None,
            updated_at: None,
            last_active: None,
        }];
        let sessions = live_sessions_from_cli_rows("hermes", rows, now, DEFAULT_STALENESS);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_id, "hermes");
        // No recency column or absolute timestamp → derived from the session id.
        assert_eq!(sessions[0].updated_at, hermes_id_timestamp(&sid).unwrap());
        assert_eq!(sessions[0].activity, ActivityState::Idle);
        assert!(sessions[0].path.is_none());
    }

    #[test]
    fn cli_rows_use_last_active_for_freshness_and_activity() {
        // The listing's recency column is the honest freshness signal: it
        // drives `updated_at`, staleness, sorting, and the working/idle
        // activity (a "now" session is Working; an "8m ago" one is Idle).
        let now = Utc::now();
        let rows = vec![
            CliListRow {
                session_id: "20260816_010000_active".into(),
                title: None,
                updated_at: None,
                last_active: Some(5),
            },
            CliListRow {
                session_id: "20260816_020000_quiet".into(),
                title: None,
                updated_at: None,
                last_active: Some(8 * 60),
            },
        ];
        let sessions = live_sessions_from_cli_rows("hermes", rows, now, DEFAULT_STALENESS);
        assert_eq!(sessions.len(), 2);
        // Freshest (5s ago) sorts first and is Working; the quiet one is Idle.
        assert_eq!(sessions[0].session_id, "20260816_010000_active");
        assert_eq!(sessions[0].activity, ActivityState::Working);
        assert!(now - sessions[0].updated_at <= Duration::seconds(10));
        assert_eq!(sessions[1].session_id, "20260816_020000_quiet");
        assert_eq!(sessions[1].activity, ActivityState::Idle);
    }

    #[test]
    fn hermes_id_timestamp_parses_local_time_as_utc() {
        // 2026-08-14 03:01:09 local → the same instant in UTC.
        let ts = hermes_id_timestamp("20260814_030109_6a9f8d").expect("parse");
        let local = ts.with_timezone(&chrono::Local);
        assert_eq!(local.format("%Y%m%d_%H%M%S").to_string(), "20260814_030109");
        // Malformed ids fail closed.
        assert!(hermes_id_timestamp("not-an-id").is_none());
        assert!(hermes_id_timestamp("20260814_030109").is_none());
        assert!(hermes_id_timestamp("20269999_999999_abc").is_none());
    }

    #[test]
    fn cli_rows_sort_freshest_first_by_id_timestamp() {
        // The listing gives no absolute timestamps, but the ids encode them:
        // the newest session must sort first, not last. Ids are generated
        // relative to now so the fixtures never fall outside the staleness
        // window (a hard-coded date would rot as time passes).
        let now = Utc::now();
        let mk = |hours_ago: i64| {
            let dt = chrono::Local::now() - chrono::Duration::hours(hours_ago);
            format!("{}_{}_6d7fc6", dt.format("%Y%m%d"), dt.format("%H%M%S"))
        };
        let rows = vec![
            CliListRow {
                session_id: mk(3),
                title: None,
                updated_at: None,
                last_active: None,
            },
            CliListRow {
                session_id: mk(24),
                title: None,
                updated_at: None,
                last_active: None,
            },
            CliListRow {
                session_id: mk(48),
                title: None,
                updated_at: None,
                last_active: None,
            },
        ];
        let sessions = live_sessions_from_cli_rows("hermes", rows, now, DEFAULT_STALENESS);
        // Freshest first: 3h ago, then 24h, then 48h (all within 7 days).
        let ids: Vec<String> = sessions.iter().map(|s| s.session_id.clone()).collect();
        assert_eq!(ids, vec![mk(3), mk(24), mk(48)]);
    }

    // -- helpers -----------------------------------------------------------

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn set_mtime(path: &std::path::Path, ts: DateTime<Utc>) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        let times = std::fs::FileTimes::new()
            .set_accessed(ts.into())
            .set_modified(ts.into());
        file.set_times(times).unwrap();
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn set_mtime(_path: &std::path::Path, _ts: DateTime<Utc>) {}
}
