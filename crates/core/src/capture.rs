use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// What the captured content actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    #[default]
    Text,
    Image,
    File,
    Url,
    Terminal,
    Mixed,
}

/// Where the capture came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Manual,
    Clipboard,
    Selection,
    Screenshot,
    File,
    Url,
    Terminal,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureSource {
    #[serde(rename = "type")]
    pub kind: SourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CaptureContent {
    #[serde(rename = "type")]
    pub kind: ContentKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<String>>,
}

/// The standardized internal representation of anything captured.
///
/// Serialized shape (matches the product spec):
/// ```json
/// {
///   "id": "uuid",
///   "timestamp": "...",
///   "source": { "type": "clipboard", "application": "Terminal" },
///   "content": { "type": "text", "text": "ECONNREFUSED 127.0.0.1:5432" },
///   "metadata": { "working_directory": "/Users/user/project" }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capture {
    #[serde(default)]
    pub id: String,
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub source: CaptureSource,
    #[serde(default)]
    pub content: CaptureContent,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl Capture {
    /// Creates a new capture with a fresh id and timestamp.
    pub fn new(source: CaptureSource, content: CaptureContent) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            source,
            content,
            metadata: HashMap::new(),
        }
    }

    /// Normalizes a capture that may have been deserialized without an id
    /// (e.g. built by the CLI). Idempotent.
    pub fn normalize(&mut self) {
        if self.id.is_empty() {
            self.id = Uuid::new_v4().to_string();
        }
    }

    pub fn text(source: SourceKind, text: impl Into<String>, application: Option<String>) -> Self {
        Self::new(
            CaptureSource {
                kind: source,
                application,
            },
            CaptureContent {
                kind: ContentKind::Text,
                text: Some(text.into()),
                path: None,
                items: None,
            },
        )
    }

    pub fn terminal(text: impl Into<String>, application: Option<String>) -> Self {
        Self::new(
            CaptureSource {
                kind: SourceKind::Terminal,
                application,
            },
            CaptureContent {
                kind: ContentKind::Terminal,
                text: Some(text.into()),
                path: None,
                items: None,
            },
        )
    }

    pub fn url(url: impl Into<String>) -> Self {
        Self::new(
            CaptureSource {
                kind: SourceKind::Url,
                application: None,
            },
            CaptureContent {
                kind: ContentKind::Url,
                text: Some(url.into()),
                path: None,
                items: None,
            },
        )
    }

    pub fn file(path: impl Into<String>, text: Option<String>) -> Self {
        Self::new(
            CaptureSource {
                kind: SourceKind::File,
                application: None,
            },
            CaptureContent {
                kind: ContentKind::File,
                text,
                path: Some(path.into()),
                items: None,
            },
        )
    }

    pub fn image(path: impl Into<String>) -> Self {
        Self::new(
            CaptureSource {
                kind: SourceKind::Screenshot,
                application: None,
            },
            CaptureContent {
                kind: ContentKind::Image,
                text: None,
                path: Some(path.into()),
                items: None,
            },
        )
    }

    /// A short human readable label, e.g. "Terminal error", "Screenshot".
    pub fn summary(&self) -> String {
        match self.content.kind {
            ContentKind::Text => {
                if let Some(app) = &self.source.application {
                    format!("Text from {app}")
                } else {
                    "Text".to_string()
                }
            }
            ContentKind::Terminal => "Terminal output".to_string(),
            ContentKind::Image => "Screenshot".to_string(),
            ContentKind::File => {
                let name = self
                    .content
                    .path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("file");
                format!("File: {name}")
            }
            ContentKind::Url => "URL".to_string(),
            ContentKind::Mixed => "Mixed capture".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_capture_serializes_to_spec_shape() {
        let mut capture = Capture::text(SourceKind::Clipboard, "ECONNREFUSED 127.0.0.1:5432", None);
        capture
            .metadata
            .insert("git_branch".to_string(), "feature/auth".to_string());

        let json = serde_json::to_value(&capture).unwrap();
        assert_eq!(json["source"]["type"], "clipboard");
        assert_eq!(json["content"]["type"], "text");
        assert_eq!(json["content"]["text"], "ECONNREFUSED 127.0.0.1:5432");
        assert_eq!(json["metadata"]["git_branch"], "feature/auth");
        assert!(json["id"].is_string());
        assert!(json["timestamp"].is_string());
    }

    #[test]
    fn capture_roundtrip_through_json() {
        let original = Capture::terminal("panic: out of bounds", Some("Terminal".into()));
        let json = serde_json::to_string(&original).unwrap();
        let back: Capture = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, original.id);
        assert_eq!(back.content.kind, ContentKind::Terminal);
        assert_eq!(back.content.text.as_deref(), Some("panic: out of bounds"));
        assert_eq!(back.source.application.as_deref(), Some("Terminal"));
    }

    #[test]
    fn partial_capture_deserializes_and_normalizes() {
        // The CLI sends minimal captures; the daemon fills in missing id.
        let json = r#"{"content":{"type":"file","path":"/tmp/error.log","text":"boom"}} "#;
        let mut capture: Capture = serde_json::from_str(json).unwrap();
        assert!(capture.id.is_empty());
        assert_eq!(capture.source.kind, SourceKind::Manual);
        capture.normalize();
        assert!(!capture.id.is_empty());
    }

    #[test]
    fn file_summary_uses_basename() {
        let capture = Capture::file("/deep/path/src/main.rs", Some("fn main() {}".into()));
        assert_eq!(capture.summary(), "File: main.rs");
    }

    #[test]
    fn url_capture_sets_url_kind() {
        let capture = Capture::url("https://github.com/example/issues/42");
        assert_eq!(capture.content.kind, ContentKind::Url);
        assert_eq!(capture.source.kind, SourceKind::Url);
    }
}
