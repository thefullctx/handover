use serde::{Deserialize, Serialize};

/// An action the user can take on a capture. Prompts are rendered from
/// `prompt_template` with `{CONTEXT}` substituted (see `prompt` module).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Action {
    pub fn new(
        id: &str,
        name: &str,
        description: &str,
        prompt_template: &str,
        icon: Option<&str>,
    ) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            prompt_template: prompt_template.to_string(),
            icon: icon.map(|i| i.to_string()),
        }
    }
}

/// The initial built-in actions. Configurable, not hard-coded in the UI.
pub fn builtin_actions() -> Vec<Action> {
    vec![
        Action::new(
            "fix",
            "Fix this",
            "Investigate this issue and fix it.",
            "Investigate this issue and fix it.\n\n{CONTEXT}",
            Some("🔧"),
        ),
        Action::new(
            "investigate",
            "Investigate",
            "Investigate this issue and determine the root cause. Do not modify files yet.",
            "Investigate this issue and determine the root cause. Do not modify files yet.\n\n{CONTEXT}",
            Some("🔍"),
        ),
        Action::new(
            "explain",
            "Explain",
            "Explain this clearly and identify the likely cause.",
            "Explain this clearly and identify the likely cause.\n\n{CONTEXT}",
            Some("📝"),
        ),
        Action::new(
            "write_tests",
            "Write tests",
            "Create appropriate tests for this issue.",
            "Create appropriate tests for this issue.\n\n{CONTEXT}",
            Some("🧪"),
        ),
        Action::new(
            "ask",
            "Ask",
            "Analyze the provided context and tell me what you recommend.",
            "Analyze the provided context and tell me what you recommend.\n\n{CONTEXT}",
            Some("💬"),
        ),
    ]
}

pub fn find_action<'a>(actions: &'a [Action], id: &str) -> Option<&'a Action> {
    actions.iter().find(|a| a.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_actions_have_unique_ids_and_exact_prompts() {
        let actions = builtin_actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "action ids must be unique");
        assert_eq!(actions.len(), 5);

        let fix = find_action(&actions, "fix").unwrap();
        assert_eq!(
            fix.prompt_template,
            "Investigate this issue and fix it.\n\n{CONTEXT}"
        );
        let investigate = find_action(&actions, "investigate").unwrap();
        assert!(investigate
            .prompt_template
            .contains("Do not modify files yet"));
        let ask = find_action(&actions, "ask").unwrap();
        assert!(ask.prompt_template.contains("what you recommend"));
    }
}
