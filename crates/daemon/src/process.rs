//! Live-process detection for configured agents (the status lights).
//!
//! Fail closed: any probe error reports "not running".

use std::path::Path;

/// Best-effort live-process check: is a process for this agent's command
/// running right now? The program is the first real token of the command
/// (`sh -c` wrappers are skipped, so `sh -c 'hermes …'` still finds hermes).
/// Matching is by exact process name first (`pgrep -x`, agents run directly
/// in a terminal), then by the program's basename appearing in the command
/// line (`pgrep -f` — catches python entry-point launchers like Hermes'
/// venv). Fail closed: any error or a wrapper-only command reports false.
pub(crate) fn agent_process_running(command: &str) -> bool {
    let Some(program) = handover_agents::generic_command::first_program(command) else {
        return false;
    };
    let basename = Path::new(&program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.clone());
    if basename.is_empty()
        || matches!(
            basename.as_str(),
            "sh" | "bash" | "zsh" | "python" | "python3"
                // Shell machinery that can only appear when the program
                // extraction failed to see through a wrapper/statement —
                // pgrepping these lights agents green for unrelated system
                // processes (`env` runs everywhere in shell scripts).
                | "env" | "export" | "unset" | "exec" | "cd" | "true" | "false"
        )
    {
        // A shell/interpreter/wrapper alone is not the agent itself.
        return false;
    }
    let probe = |extra: &[&str]| {
        std::process::Command::new("pgrep")
            .args(extra)
            .arg(&basename)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    // Exact process-name match first (agents run directly in a terminal);
    // then a whole-component scan for interpreter launchers (Hermes runs as
    // `…/venv/bin/python …/hermes`, where the process NAME is python). A bare
    // substring search (`pgrep -f`) must NEVER be used: it matched unrelated
    // processes (`omp` inside MTLCompilerService, `codex` inside a vite/npm
    // invocation) and lit agents green that were not running at all.
    probe(&["-x"]) || any_command_line_contains(&basename)
}

/// True when `line` contains `token` as a whole path component / whole
/// argument — never buried inside a longer word. Splitting on whitespace
/// and `/`: `MTLCompilerService` is one component (≠ `omp`), while the
/// trailing `hermes` in `…/hermes-agent/hermes` stands alone (= `hermes`).
pub(crate) fn command_line_has_component(line: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    line.split(|c: char| c.is_whitespace() || c == '/')
        .any(|part| part == token)
}

/// Scans every live process's command line for the program appearing as a
/// whole component. One `ps` call, fail closed on error.
fn any_command_line_contains(token: &str) -> bool {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "command="])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| command_line_has_component(line, token))
}
