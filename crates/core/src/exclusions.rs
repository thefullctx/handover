use std::path::Path;

/// Default paths that must never be attached to a handoff automatically.
///
/// Mirrors the product spec:
/// `.env`, `*.pem`, `*.key`, `credentials*`, `~/.ssh/*` and a few friends.
pub fn default_excluded_paths() -> Vec<String> {
    vec![
        ".env".to_string(),
        ".env.*".to_string(),
        "*.pem".to_string(),
        "*.key".to_string(),
        "*.p12".to_string(),
        "*.pfx".to_string(),
        "credentials*".to_string(),
        "id_rsa".to_string(),
        "id_ed25519".to_string(),
        "~/.ssh/*".to_string(),
    ]
}

/// Minimal glob matcher supporting `*` (any characters), `**` (same, kept for
/// familiarity) and `?` (a single character). Matching is **case-sensitive**.
///
/// Note: pathological patterns like `*a*a*a*…` against long names can be
/// exponential. Handover only uses short privacy patterns (`.env`, `*.pem`, …),
/// so this is not a practical concern in-tree.
pub fn matches_glob(pattern: &str, text: &str) -> bool {
    matches_glob_raw(pattern, text)
}

/// Case-insensitive glob match (used for privacy exclusions so `Secret.PEM`
/// cannot bypass `*.pem` on case-insensitive filesystems like default macOS APFS).
pub fn matches_glob_ci(pattern: &str, text: &str) -> bool {
    matches_glob_raw(&pattern.to_lowercase(), &text.to_lowercase())
}

fn matches_glob_raw(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        if p.is_empty() {
            return t.is_empty();
        }
        match p[0] {
            '*' => {
                if p.len() >= 2 && p[1] == '*' {
                    go(&p[2..], t) || (!t.is_empty() && go(p, &t[1..]))
                } else {
                    go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..]))
                }
            }
            '?' => !t.is_empty() && go(&p[1..], &t[1..]),
            c => !t.is_empty() && t[0] == c && go(&p[1..], &t[1..]),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t)
}

/// Returns true if `path` matches any of the exclusion patterns.
///
/// Patterns without a `/` match the file name; patterns with a `/` (e.g.
/// `~/.ssh/*`) match the full path. A leading `~/` is expanded to the user's
/// home directory.
///
/// Matching is **case-insensitive** so privacy exclusions cannot be bypassed
/// via `Secret.PEM` / `.ENV` on case-insensitive volumes.
pub fn is_excluded_path(path: &Path, patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().trim_end_matches('/').to_string())
        .unwrap_or_default();

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    for raw in patterns {
        let pattern = raw.trim();
        if pattern.is_empty() {
            continue;
        }
        // A `~/`-rooted pattern with no resolvable home would expand to
        // `/.ssh/*` and never match (or worse, match the wrong root).
        // Skip it instead of guessing.
        if (pattern == "~" || pattern.starts_with("~/")) && home.is_empty() {
            continue;
        }
        let expanded = if pattern == "~" {
            home.clone()
        } else if let Some(rest) = pattern.strip_prefix("~/") {
            format!("{}/{}", home, rest.trim_start_matches('/'))
        } else {
            pattern.to_string()
        };

        if matches_glob_ci(&expanded, &path_str) {
            return true;
        }
        if !expanded.contains('/') && matches_glob_ci(&expanded, name) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(matches_glob("*.pem", "id_rsa.pem"));
        assert!(!matches_glob("*.pem", "id_rsa.pem.backup"));
        assert!(matches_glob("*.key", "a.b.key"));
        assert!(matches_glob("config?", "config1"));
        assert!(!matches_glob("config?", "config10"));
        assert!(matches_glob("a/**/c", "a/b/c"));
    }

    #[test]
    fn excludes_env_files_anywhere() {
        assert!(is_excluded_path(Path::new("/x/y/.env"), &[".env".into()]));
        assert!(is_excluded_path(Path::new(".env"), &[".env".into()]));
        assert!(is_excluded_path(
            Path::new("/x/.env.production"),
            &[".env.*".into()]
        ));
        assert!(!is_excluded_path(
            Path::new("/x/src/lib.rs"),
            &[".env".into()]
        ));
    }

    #[test]
    fn excludes_pem_and_key_files() {
        let patterns = [".env".into(), "*.pem".into(), "*.key".into()];
        assert!(is_excluded_path(
            Path::new("/a/credentials/server.pem"),
            &patterns
        ));
        assert!(is_excluded_path(Path::new("/a/deploy.key"), &patterns));
        assert!(!is_excluded_path(Path::new("/a/keyboard.rs"), &patterns));
    }

    #[test]
    fn excludes_are_case_insensitive() {
        let patterns = default_excluded_paths();
        assert!(is_excluded_path(Path::new("/x/Y/.ENV"), &patterns));
        assert!(is_excluded_path(Path::new("/x/Secret.PEM"), &patterns));
        assert!(is_excluded_path(Path::new("/x/Deploy.KEY"), &patterns));
        assert!(is_excluded_path(
            Path::new("/x/CREDENTIALS.json"),
            &patterns
        ));
        assert!(is_excluded_path(Path::new("/x/Id_Rsa"), &patterns));
    }

    #[test]
    fn excludes_ssh_directory_via_home_expansion() {
        let home = dirs::home_dir().unwrap();
        let ssh_path = home.join(".ssh/id_rsa");
        assert!(is_excluded_path(&ssh_path, &["~/.ssh/*".into()]));
    }

    #[test]
    fn excludes_credentials_pattern() {
        let patterns = ["credentials*".into()];
        assert!(is_excluded_path(
            Path::new("/tmp/credentials_backup.txt"),
            &patterns
        ));
        assert!(!is_excluded_path(Path::new("/tmp/creds.txt"), &patterns));
    }

    #[test]
    fn default_list_never_matches_normal_source_files() {
        let defaults = default_excluded_paths();
        assert!(!is_excluded_path(Path::new("/x/src/main.rs"), &defaults));
        assert!(!is_excluded_path(Path::new("/x/Cargo.toml"), &defaults));
    }
}
