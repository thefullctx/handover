//! Adapter compatibility declarations.
//!
//! An "adapter" in Handover is not code — it is the set of assumptions Handover
//! makes about one agent's CLI, spread across five tables keyed by the agent id
//! string: the session catalog (`builtin_session_agents`), the gallery catalog
//! (`agent_catalog`), the session-id rules (`core::session::session_id_from_filename`),
//! the rotated-id shapes (`agents::session_agent::matches_id_shape`) and the
//! provider-config locations (`daemon::provider_health::sniff_endpoint`).
//!
//! When an agent changes its CLI, those assumptions go stale *silently*: a
//! renamed resume flag or a changed session filename still exits 0, so the
//! handoff reports success while the context landed in the wrong conversation.
//! Nothing in the architecture can detect that automatically without probing
//! every agent's `--version` on every status poll.
//!
//! This module is the cheap half of the answer: a declaration of which agent
//! version each set of assumptions was verified against, so drift is reported
//! with evidence ("verified against 0.160.0" next to "I have 0.163.0") rather
//! than guessed at. It is deliberately a plain table — no version ranges, no
//! resolver, no dependency management. If the CLI contract changes, the
//! adapter is updated *and* this entry is updated in the same change; the
//! tests below fail if an adapter exists without a declaration.
//!
//! **This is evidence, not a guarantee.** It records what was true on
//! `verified_on`, on one machine, for one agent version. It is NOT checked
//! against the agent the user has installed, and Handover does not refuse to
//! run when they differ — an installed agent may well be newer or older than
//! this says, and that mismatch is exactly what a drift report needs to
//! surface. Treating this table as a promise that the installed agent always
//! matches would defeat its only purpose.
//!
//! Verified on-machine by running each agent's `--version` and
//! `scripts/probe-sessions.sh` (filenames + mtimes only).

/// What Handover assumes about one agent's CLI, and when that was last checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCompat {
    /// Agent id — the same key used by every adapter table.
    pub agent_id: &'static str,
    /// The agent version the assumptions below were verified against, or
    /// `None` when they have never been verified on a real install (best
    /// effort, or the agent could not be run). `None` is honest and useful:
    /// it is what a bug report should be weighed against.
    ///
    /// A historical record, never validated at runtime.
    pub verified_agent_version: Option<&'static str>,
    /// ISO date (YYYY-MM-DD) of that verification — the day the claim above
    /// was true, not a promise about today.
    pub verified_on: &'static str,
    /// The exact surface that breaks when the agent changes. Keep this
    /// specific — it is what a future contributor re-checks first.
    pub assumes: &'static str,
}

impl AdapterCompat {
    /// One-line summary for `AgentMeta` / `handover agents`. Shown as
    /// `adapter: <version> (verified <date>)`, or an explicit "unverified"
    /// marker when no version was ever confirmed.
    pub fn summary(&self) -> String {
        match self.verified_agent_version {
            Some(version) => format!("adapter: {version} (verified {})", self.verified_on),
            None => format!(
                "adapter: unverified (assumptions from {})",
                self.verified_on
            ),
        }
    }
}

/// The declaration for every agent Handover ships an adapter for.
///
/// A new catalog entry — or a new arm in `session_id_from_filename`,
/// `matches_id_shape` or `sniff_endpoint` — must be added here too; the
/// drift-guard tests fail otherwise.
pub const ADAPTER_COMPAT: &[AdapterCompat] = &[
    AdapterCompat {
        agent_id: "claude",
        verified_agent_version: Some("2.1.288"),
        verified_on: "2026-10-04",
        assumes: "sessions at ~/.claude/projects/<proj>/<uuid>.jsonl (filename IS the \
                  session id); resume via `claude -p \"{PROMPT}\" --resume <id>`; \
                  `-p` streams the reply",
    },
    AdapterCompat {
        agent_id: "codex",
        verified_agent_version: Some("codex-cli 0.160.0"),
        verified_on: "2026-10-04",
        assumes: "sessions at ~/.codex/sessions/<Y>/<M>/<D>/rollout-<ts>-<uuid>.jsonl \
                  (id is the trailing uuid after `rollout-`); resume via \
                  `codex exec resume <id>`; headless exec refuses outside a git \
                  repo, hence --skip-git-repo-check",
    },
    AdapterCompat {
        agent_id: "droid",
        verified_agent_version: Some("0.233.0"),
        verified_on: "2026-10-04",
        assumes: "sessions at ~/.factory/sessions/<enc-cwd>/<uuid>.jsonl (filename IS \
                  the session id); resume via `droid exec -s <id> \"{PROMPT}\"`",
    },
    AdapterCompat {
        agent_id: "omp",
        verified_agent_version: Some("18.5.0"),
        verified_on: "2026-10-04",
        assumes: "sessions at ~/.omp/agent/sessions/<enc-cwd>/<ts>_<sessionId>.jsonl \
                  (id is the part after the last `_`); resume via \
                  `omp -p \"{PROMPT}\" -r <id>`",
    },
    AdapterCompat {
        agent_id: "opencode",
        verified_agent_version: Some("2.0.19"),
        verified_on: "2026-10-04",
        assumes: "one-shot send only — no session adapter; model endpoint in \
                  ~/.config/opencode/opencode.json(c) as `baseURL`",
    },
    // Hermes could not be run on the verification machine: its launcher points
    // at a deleted virtualenv, so `hermes --version` and `hermes sessions list`
    // both fail. The assumptions below come from the 2026-08-14 probe and are
    // explicitly NOT re-verified — hence `None` rather than a guessed version.
    AdapterCompat {
        agent_id: "hermes",
        verified_agent_version: None,
        verified_on: "2026-08-14",
        assumes: "sessions live in SQLite (~/.hermes/state.db, no filesystem glob) \
                  and are listed with `hermes sessions list`; ids are \
                  <YYYYMMDD>_<HHMMSS>_<hex> and encode their own timestamp; resume \
                  via `hermes chat -q \"{PROMPT}\" --resume <id>`",
    },
];

/// The compatibility declaration for `agent_id`, if Handover ships an adapter
/// for it. `None` for a user's own command agent — those carry no bundled
/// assumptions, so there is nothing to drift.
pub fn compat_for(agent_id: &str) -> Option<&'static AdapterCompat> {
    ADAPTER_COMPAT.iter().find(|c| c.agent_id == agent_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_adapter_declares_its_assumptions_and_a_date() {
        for compat in ADAPTER_COMPAT {
            assert!(
                !compat.assumes.trim().is_empty(),
                "{}: an adapter must name the surface it assumes",
                compat.agent_id
            );
            assert!(
                !compat.agent_id.trim().is_empty(),
                "an adapter entry needs an agent id"
            );
            let date = compat.verified_on;
            assert_eq!(
                date.len(),
                10,
                "{}: verified_on must be an ISO date (YYYY-MM-DD), got `{date}`",
                compat.agent_id
            );
            let bytes = date.as_bytes();
            assert!(
                bytes[4] == b'-' && bytes[7] == b'-' && bytes[..4].iter().all(u8::is_ascii_digit),
                "{}: verified_on must be an ISO date (YYYY-MM-DD), got `{date}`",
                compat.agent_id
            );
        }
    }

    #[test]
    fn agent_ids_are_unique() {
        let mut ids: Vec<&str> = ADAPTER_COMPAT.iter().map(|c| c.agent_id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "duplicate agent_id in ADAPTER_COMPAT — one declaration per adapter"
        );
    }

    #[test]
    fn session_id_rule_agents_all_declare_compat() {
        // Third drift surface: `core::session::session_id_from_filename` keys
        // per-agent filename→id rules off the agent id. Every id with a
        // non-default rule must be declared, so adding an arm there without a
        // declaration fails CI instead of drifting silently.
        for agent_id in handover_core::session::SESSION_ID_RULE_AGENTS {
            assert!(
                compat_for(agent_id).is_some(),
                "{agent_id}: has a custom session-id rule but no ADAPTER_COMPAT entry"
            );
        }
    }

    #[test]
    fn lookup_finds_by_id_and_misses_cleanly() {
        assert_eq!(compat_for("codex").unwrap().agent_id, "codex");
        // Agents with no bundled adapter (a user's own command agent) have no
        // declaration — and must not be invented for them.
        assert!(compat_for("my-own-agent").is_none());
    }

    #[test]
    fn unverified_adapters_say_so_in_the_summary() {
        // An unverified adapter must never read like a verified one.
        let hermes = compat_for("hermes").expect("hermes adapter");
        assert_eq!(hermes.verified_agent_version, None);
        let summary = hermes.summary();
        assert!(
            summary.contains("unverified"),
            "unverified adapter must say so: {summary}"
        );

        let codex = compat_for("codex").expect("codex adapter");
        let summary = codex.summary();
        assert!(summary.contains("0.160.0"), "{summary}");
        assert!(summary.contains("2026-10-04"), "{summary}");
        assert!(!summary.contains("unverified"), "{summary}");
    }
}
