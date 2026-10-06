//! Curated catalog of known agents for the Settings gallery.
//!
//! The gallery lets users add a supported agent with one click: Handover
//! detects the installed binary and writes the recommended `command` into
//! the config itself. Local agents are one-shot CLI commands built from a
//! `{BIN}` + `{PROMPT}` template; cloud agents (API-key based) are a future
//! extension of this module.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// A known agent shown in the gallery.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Command template. `{BIN}` is replaced with the resolved binary path,
    /// `{PROMPT}` with the rendered prompt (handled by the generic agent).
    pub command_template: String,
    /// Bare binary name used for PATH lookup, e.g. `"hermes"`.
    pub binary_name: String,
    /// Likely absolute paths (may use `~`) checked before PATH lookup.
    pub candidate_paths: Vec<String>,
    pub timeout_secs: u64,
    /// Home-relative credential file/directory used as a read-only
    /// "signed in?" probe (e.g. Codex writes `~/.codex/auth.json` on login).
    /// `None` for agents that need no login (or have no reliable marker).
    pub auth_marker: Option<String>,
}

/// The agents users can pick from in the gallery.
pub fn agent_catalog() -> Vec<CatalogAgent> {
    vec![
        CatalogAgent {
            id: "hermes".into(),
            name: "Hermes".into(),
            description: "Local coding agent with tool-calling (one-shot).".into(),
            command_template: "{BIN} -z \"{PROMPT}\"".into(),
            binary_name: "hermes".into(),
            candidate_paths: vec![
                "~/.local/bin/hermes".into(),
                "~/bin/hermes".into(),
                "/opt/homebrew/bin/hermes".into(),
                "/usr/local/bin/hermes".into(),
            ],
            timeout_secs: 300,
            // Hermes keeps its own config; presence implies setup.
            auth_marker: Some("~/.hermes/config.yaml".into()),
        },
        CatalogAgent {
            id: "codex".into(),
            name: "Codex".into(),
            description: "OpenAI Codex CLI (non-interactive).".into(),
            // --skip-git-repo-check: headless exec refuses to run outside a
            // git repo (the trusted-projects list does not satisfy this
            // check), which made every handoff from a non-repo cwd die in
            // milliseconds. Handover targets sessions explicitly, so skip.
            command_template: "{BIN} exec --skip-git-repo-check \"{PROMPT}\"".into(),
            binary_name: "codex".into(),
            candidate_paths: vec![
                "~/.local/bin/codex".into(),
                "~/bin/codex".into(),
                "/opt/homebrew/bin/codex".into(),
                "/usr/local/bin/codex".into(),
            ],
            timeout_secs: 300,
            auth_marker: Some("~/.codex/auth.json".into()),
        },
        CatalogAgent {
            id: "opencode".into(),
            name: "OpenCode".into(),
            description: "OpenCode CLI (non-interactive).".into(),
            command_template: "{BIN} run \"{PROMPT}\"".into(),
            binary_name: "opencode".into(),
            candidate_paths: vec![
                "~/.opencode/bin/opencode".into(),
                "~/.local/bin/opencode".into(),
                "~/bin/opencode".into(),
                "/opt/homebrew/bin/opencode".into(),
                "/usr/local/bin/opencode".into(),
            ],
            timeout_secs: 300,
            auth_marker: Some("~/.local/share/opencode/auth.json".into()),
        },
        CatalogAgent {
            id: "omp".into(),
            name: "OMP".into(),
            description: "OMP coding agent (non-interactive, -p).".into(),
            command_template: "{BIN} -p \"{PROMPT}\"".into(),
            binary_name: "omp".into(),
            candidate_paths: vec![
                "~/.local/bin/omp".into(),
                "~/bin/omp".into(),
                "/opt/homebrew/bin/omp".into(),
                "/usr/local/bin/omp".into(),
            ],
            timeout_secs: 300,
            auth_marker: None,
        },
    ]
}

/// A gallery entry enriched with the user's actual state.
#[derive(Debug, Clone, Serialize)]
pub struct GalleryAgent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub command_template: String,
    pub binary_name: String,
    pub timeout_secs: u64,
    /// Resolved binary path when the agent is installed (or user-provided).
    pub detected_path: Option<String>,
    /// True when the agent is already present in the user's config.
    pub configured: bool,
    /// Read-only sign-in probe: `Some(true)` when the agent's credential
    /// marker exists (signed in), `Some(false)` when it does not, and
    /// `None` when the agent has no known marker (unknown / needs none).
    pub signed_in: Option<bool>,
}

/// Builds the gallery, resolving each agent's binary and marking which ones
/// are already configured.
pub fn gallery(configured_ids: &[String]) -> Vec<GalleryAgent> {
    agent_catalog()
        .into_iter()
        .map(|a| {
            let detected_path = resolve_binary(&a, None).map(|p| p.to_string_lossy().into_owned());
            let signed_in = a
                .auth_marker
                .as_deref()
                .map(|marker| expand_home(marker).exists());
            GalleryAgent {
                configured: configured_ids.iter().any(|c| c == &a.id),
                detected_path,
                signed_in,
                id: a.id,
                name: a.name,
                description: a.description,
                command_template: a.command_template,
                binary_name: a.binary_name,
                timeout_secs: a.timeout_secs,
            }
        })
        .collect()
}

/// Resolves the binary for an agent: an explicit hint first, then the
/// candidate paths, then a PATH lookup for the bare name. Returns the
/// absolute path when found and executable.
pub fn resolve_binary(agent: &CatalogAgent, hint: Option<&str>) -> Option<PathBuf> {
    resolve_binary_in(agent, hint, std::env::var_os("PATH").as_deref())
}

/// [`resolve_binary`] against an explicit `PATH` value. Tests use this
/// instead of mutating the process-global `PATH`, which other tests in the
/// same binary read concurrently when they spawn `sh`.
fn resolve_binary_in(
    agent: &CatalogAgent,
    hint: Option<&str>,
    path_var: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    if let Some(hint) = hint {
        let hint = hint.trim();
        if !hint.is_empty() {
            if let Some(p) = resolve_one(hint) {
                return Some(p);
            }
            // A bare name (e.g. `omp`) → PATH lookup.
            if !hint.contains('/') {
                if let Some(p) = find_on_path(hint, path_var) {
                    return Some(p);
                }
            }
        }
    }
    for candidate in &agent.candidate_paths {
        if let Some(p) = resolve_one(candidate) {
            return Some(p);
        }
    }
    find_on_path(&agent.binary_name, path_var)
}

/// Checks a single path (expanding `~`), returning it when it exists and is
/// executable.
fn resolve_one(path: &str) -> Option<PathBuf> {
    let expanded = expand_home(path);
    if is_executable_file(&expanded) {
        Some(expanded)
    } else {
        None
    }
}

/// Searches the `PATH` value `path_var` for an executable named `name`.
fn find_on_path(name: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var?) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path
            .metadata()
            .map(|m| m.permissions().mode() & 0o111)
            .unwrap_or(0)
            == 0
        {
            return false;
        }
    }
    true
}

/// Replaces `{BIN}` in the template with the resolved binary. Paths
/// containing whitespace are quoted so `sh -c` parses them as one word, and
/// the content is escaped for double-quote context (`$`, backticks,
/// backslashes) — mirroring the generic command agent's prompt escaping.
pub fn build_command(template: &str, bin: &str) -> String {
    let quoted = if needs_quoting(bin) {
        let escaped = bin
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('`', "\\`");
        format!("\"{escaped}\"")
    } else {
        bin.to_string()
    };
    template.replace("{BIN}", &quoted)
}

/// A binary path needs shell quoting when it contains characters that `sh -c`
/// would interpret — whitespace, quotes, or expansion metacharacters.
fn needs_quoting(bin: &str) -> bool {
    bin.chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\' | '$' | '`'))
}

/// Resolves a user-supplied path or bare name **strictly** — no candidate/
/// PATH fallback. An explicit-but-wrong input must surface as an error, not
/// silently configure a different binary than the user typed.
pub fn resolve_explicit(input: &str) -> Option<PathBuf> {
    if let Some(p) = resolve_one(input) {
        return Some(p);
    }
    if !input.contains('/') {
        return find_on_path(input, std::env::var_os("PATH").as_deref());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_known_agents() {
        let catalog = agent_catalog();
        let ids: Vec<&str> = catalog.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["hermes", "codex", "opencode", "omp"]);
    }

    #[test]
    fn every_gallery_agent_declares_adapter_compat() {
        // A gallery entry writes a `command` into the user's config from an
        // agent-specific template, so it carries the same drift risk as the
        // session catalog: adding one without a declaration must fail here.
        for agent in agent_catalog() {
            assert!(
                handover_config::compat_for(&agent.id).is_some(),
                "{}: a gallery adapter needs an ADAPTER_COMPAT entry \
                 (agent id, verified version/date, and the surface it assumes)",
                agent.id
            );
        }
    }

    #[test]
    fn no_orphan_adapter_compat_declarations() {
        // The reverse direction: a declaration for an agent Handover ships no
        // adapter for. That is stale evidence — it keeps printing an
        // "adapter: …" line for an agent with no adapter and hides the fact
        // that the adapter was removed. This lives in the daemon because it is
        // the only crate that can see both catalogs at once.
        let gallery = agent_catalog();
        let sessions = handover_config::builtin_session_agents();
        let mut bundled: Vec<&str> = gallery.iter().map(|a| a.id.as_str()).collect();
        bundled.extend(sessions.iter().map(|a| a.id.as_str()));
        for compat in handover_config::ADAPTER_COMPAT {
            assert!(
                bundled.contains(&compat.agent_id),
                "{}: declared in ADAPTER_COMPAT but is not a bundled adapter — \
                 remove the declaration or restore the adapter",
                compat.agent_id
            );
        }
    }

    #[test]
    fn build_command_substitutes_binary() {
        let cmd = build_command("{BIN} -z \"{PROMPT}\"", "/Users/user/.local/bin/hermes");
        assert_eq!(cmd, "/Users/user/.local/bin/hermes -z \"{PROMPT}\"");
    }

    #[test]
    fn build_command_quotes_paths_with_spaces() {
        let cmd = build_command("{BIN} run \"{PROMPT}\"", "/Users/user/My Apps/opencode");
        assert_eq!(cmd, "\"/Users/user/My Apps/opencode\" run \"{PROMPT}\"");
    }

    #[test]
    fn build_command_escapes_shell_metachars_in_quoted_paths() {
        // `$` and backticks must survive `sh -c` unexpanded inside the quotes.
        let cmd = build_command("{BIN} \"{PROMPT}\"", "/Users/x/my$dir/`weird`/omp");
        assert_eq!(cmd, "\"/Users/x/my\\$dir/\\`weird\\`/omp\" \"{PROMPT}\"");
    }

    #[test]
    fn resolves_fake_binary_via_path() {
        // An explicit PATH, not `set_var`: replacing the process PATH with a
        // dir that lacks /bin made concurrent tests fail to spawn `sh`.
        let dir = std::env::temp_dir().join(format!("ho-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("fake-agent");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let path = Some(dir.as_os_str());
        let agent = CatalogAgent {
            id: "fake".into(),
            name: "Fake".into(),
            description: String::new(),
            command_template: "{BIN} \"{PROMPT}\"".into(),
            binary_name: "fake-agent".into(),
            candidate_paths: vec![],
            timeout_secs: 60,
            auth_marker: None,
        };
        assert_eq!(
            resolve_binary_in(&agent, None, path).as_deref(),
            Some(bin.as_path())
        );
        // A wrong explicit hint falls back to candidates + PATH.
        assert_eq!(
            resolve_binary_in(&agent, Some("/nonexistent/fake-agent"), path).as_deref(),
            Some(bin.as_path())
        );
        // An explicit valid path wins.
        assert_eq!(
            resolve_binary_in(&agent, Some(bin.to_str().unwrap()), path).as_deref(),
            Some(bin.as_path())
        );
        // Without a PATH the bare name is not found.
        assert!(resolve_binary_in(&agent, None, None).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolves_tilde_candidate_path() {
        // HOME has no injectable equivalent here (`~` expands via the real
        // home dir), so take the crate-wide env lock and restore-on-Drop guard
        // shared with the lib.rs tests that also change HOME.
        let _lock = crate::tests::lock_env();
        let _env = crate::tests::EnvGuard::capture();
        let dir = std::env::temp_dir().join(format!("ho-catalog-home-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("fake-bin");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        std::env::set_var("HOME", &dir);
        let agent = CatalogAgent {
            id: "fake".into(),
            name: "Fake".into(),
            description: String::new(),
            command_template: "{BIN} \"{PROMPT}\"".into(),
            binary_name: "fake-bin".into(),
            candidate_paths: vec!["~/fake-bin".into()],
            timeout_secs: 60,
            auth_marker: None,
        };
        assert_eq!(resolve_binary(&agent, None).as_deref(), Some(bin.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_binary_reports_none() {
        let agent = CatalogAgent {
            id: "ghost".into(),
            name: "Ghost".into(),
            description: String::new(),
            command_template: "{BIN} \"{PROMPT}\"".into(),
            binary_name: "definitely-not-a-real-agent-xyz".into(),
            candidate_paths: vec![],
            timeout_secs: 60,
            auth_marker: None,
        };
        assert!(resolve_binary(&agent, None).is_none());
        // An explicit wrong path must not silently fall back to anything.
        assert!(resolve_explicit("/nonexistent/fake-agent").is_none());
    }
}
