use crate::action::Action;
use crate::capture::{Capture, ContentKind};

const METADATA_LABELS: [(&str, &str); 6] = [
    ("working_directory", "Working directory"),
    ("git_branch", "Git branch"),
    ("git_status", "Git status"),
    ("application", "Source application"),
    // Set by the daemon when a file is path-only (too large, binary, non-UTF-8).
    ("file_note", "File note"),
    // Set when clipboard/CLI/stdin text is truncated to the capture size cap.
    ("text_note", "Text note"),
];

/// Renders the human readable context for a capture: its content plus any
/// relevant metadata (working directory, git branch, ...).
pub fn render_capture_context(capture: &Capture) -> String {
    let mut parts: Vec<String> = Vec::new();

    let content = match capture.content.kind {
        ContentKind::Text | ContentKind::Terminal | ContentKind::Url => {
            capture.content.text.clone()
        }
        ContentKind::File => match (&capture.content.path, &capture.content.text) {
            (Some(p), Some(t)) => Some(format!("File: {p}\n\n{t}")),
            (Some(p), None) => Some(format!("File: {p}")),
            _ => None,
        },
        ContentKind::Image => match (&capture.content.path, &capture.content.text) {
            (Some(p), Some(t)) => Some(format!("Attached image: {p}\n\n{t}")),
            (Some(p), None) => Some(format!("Attached image: {p}")),
            _ => None,
        },
        ContentKind::Mixed => capture.content.items.as_ref().map(|items| items.join("\n")),
    };

    if let Some(c) = content {
        if !c.trim().is_empty() {
            parts.push(c.trim_end().to_string());
        }
    }

    let mut meta: Vec<String> = Vec::new();
    for (key, label) in METADATA_LABELS {
        if let Some(value) = capture.metadata.get(key) {
            if !value.is_empty() {
                meta.push(format!("- {label}: {value}"));
            }
        }
    }
    if !meta.is_empty() {
        parts.push(format!("Context:\n{}", meta.join("\n")));
    }

    parts.join("\n\n")
}

/// Renders the final prompt for an action + capture.
///
/// `{CONTEXT}` in the template is replaced with the rendered context. If the
/// template has no placeholder, the context is appended with a separator.
pub fn render_action_prompt(action: &Action, capture: &Capture) -> String {
    let context = render_capture_context(capture);
    if action.prompt_template.contains("{CONTEXT}") {
        action.prompt_template.replace("{CONTEXT}", &context)
    } else if context.trim().is_empty() {
        action.prompt_template.trim_end().to_string()
    } else {
        format!(
            "{}\n\nContext:\n{}",
            action.prompt_template.trim_end(),
            context
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{builtin_actions, find_action};
    use crate::capture::{Capture, SourceKind};

    fn error_capture() -> Capture {
        let mut capture = Capture::terminal("ECONNREFUSED 127.0.0.1:5432", Some("Terminal".into()));
        capture
            .metadata
            .insert("working_directory".into(), "/Users/user/proj".into());
        capture
            .metadata
            .insert("git_branch".into(), "feature/auth".into());
        capture
    }

    #[test]
    fn context_includes_content_and_metadata() {
        let ctx = render_capture_context(&error_capture());
        assert!(ctx.contains("ECONNREFUSED 127.0.0.1:5432"));
        assert!(ctx.contains("Working directory: /Users/user/proj"));
        assert!(ctx.contains("Git branch: feature/auth"));
    }

    #[test]
    fn fix_prompt_contains_instruction_and_context() {
        let actions = builtin_actions();
        let fix = find_action(&actions, "fix").unwrap();
        let prompt = render_action_prompt(fix, &error_capture());
        assert!(prompt.contains("Investigate this issue and fix it."));
        assert!(prompt.contains("ECONNREFUSED 127.0.0.1:5432"));
    }

    #[test]
    fn template_without_placeholder_gets_context_appended() {
        let action =
            crate::action::Action::new("custom", "Custom", "", "Do something interesting.", None);
        let prompt = render_action_prompt(&action, &error_capture());
        assert!(prompt.starts_with("Do something interesting."));
        assert!(prompt.contains("Context:"));
        assert!(prompt.contains("ECONNREFUSED"));
    }

    #[test]
    fn empty_context_stays_tidy() {
        let capture = Capture::text(SourceKind::Manual, "   ", None);
        let actions = builtin_actions();
        let ask = find_action(&actions, "ask").unwrap();
        let prompt = render_action_prompt(ask, &capture);
        assert!(!prompt.contains("Context:"));
        assert_eq!(
            prompt.trim_end(),
            "Analyze the provided context and tell me what you recommend."
        );
    }

    #[test]
    fn file_note_reaches_the_prompt() {
        let mut capture = Capture::file("/tmp/huge.log", None);
        capture.metadata.insert(
            "file_note".into(),
            "file is 2.0 MiB and was not attached (max 1 MiB)".into(),
        );
        let actions = builtin_actions();
        let ask = find_action(&actions, "ask").unwrap();
        let prompt = render_action_prompt(ask, &capture);
        assert!(
            prompt.contains("File note:"),
            "file_note must be labelled in the prompt: {prompt}"
        );
        assert!(
            prompt.contains("was not attached"),
            "file_note body must appear in the prompt: {prompt}"
        );
        assert!(prompt.contains("File: /tmp/huge.log"));
    }

    #[test]
    fn text_note_reaches_the_prompt() {
        let mut capture = Capture::text(SourceKind::Clipboard, "truncated body…", None);
        capture.metadata.insert(
            "text_note".into(),
            "text was truncated to 1.0 MiB (original was larger)".into(),
        );
        let ctx = render_capture_context(&capture);
        assert!(ctx.contains("Text note:"));
        assert!(ctx.contains("truncated"));
    }
}
