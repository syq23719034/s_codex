//! Hidden user-context fragment for extension-owned model steering.

use super::ContextualUserFragment;
use super::UserGoalUpdate;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::input_text;
use std::error::Error;
use std::fmt;

const CONTEXT_START_MARKER: &str = "<codex_internal_context";
const CONTEXT_END_MARKER: &str = "</codex_internal_context>";
const LEGACY_GOAL_CONTEXT_START_MARKER: &str = "<goal_context>";
const LEGACY_GOAL_CONTEXT_END_MARKER: &str = "</goal_context>";
const SOURCE_ATTR_START: &str = " source=\"";
const SOURCE_ATTR_END: &str = "\">";

/// Source label for hidden internal model context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalContextSource(String);

impl InternalContextSource {
    /// Creates a source label for an internal model-context fragment.
    ///
    /// Sources are intentionally constrained so the value can be embedded in the
    /// wrapper without escaping and still remain easy to audit in stored history.
    pub fn new(source: impl Into<String>) -> Result<Self, InvalidInternalContextSource> {
        let source = source.into();
        if is_valid_source(&source) {
            Ok(Self(source))
        } else {
            Err(InvalidInternalContextSource { source })
        }
    }

    /// Creates a source label from a trusted static string.
    pub fn from_static(source: &'static str) -> Self {
        Self::new(source)
            .unwrap_or_else(|_| panic!("invalid static internal context source: {source}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Error returned when an internal model-context source is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidInternalContextSource {
    source: String,
}

impl fmt::Display for InvalidInternalContextSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = &self.source;
        write!(
            f,
            "invalid internal model context source {source:?}; expected [a-z][a-z0-9_]*"
        )
    }
}

impl Error for InvalidInternalContextSource {}

/// Hidden runtime-owned context injected into model input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalModelContextFragment {
    source: InternalContextSource,
    body: String,
}

impl InternalModelContextFragment {
    /// Creates hidden model context with an extension-owned source label.
    pub fn new(source: InternalContextSource, body: impl Into<String>) -> Self {
        Self {
            source,
            body: body.into(),
        }
    }
}

impl ContextualUserFragment for InternalModelContextFragment {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind(format!("{}.internal_context", self.source.as_str()))
    }

    fn role(&self) -> &'static str {
        "user"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (CONTEXT_START_MARKER, CONTEXT_END_MARKER)
    }

    fn matches_text(text: &str) -> bool {
        let trimmed = text.trim();
        if matches_legacy_goal_context(trimmed) {
            return true;
        }

        let Some(rest) = trimmed.strip_prefix(CONTEXT_START_MARKER) else {
            return false;
        };
        let Some(rest) = rest.strip_prefix(SOURCE_ATTR_START) else {
            return false;
        };
        let Some((source, body_and_close)) = rest.split_once(SOURCE_ATTR_END) else {
            return false;
        };

        is_valid_source(source) && body_and_close.ends_with(CONTEXT_END_MARKER)
    }

    fn body(&self) -> ContentItem {
        let source = self.source.as_str();
        let body = &self.body;
        input_text(format!(" source=\"{source}\">\n{body}\n"))
    }
}

fn matches_legacy_goal_context(text: &str) -> bool {
    text.starts_with(LEGACY_GOAL_CONTEXT_START_MARKER)
        && text.ends_with(LEGACY_GOAL_CONTEXT_END_MARKER)
}

/// Goal steering cannot be reconstructed from the canonical session prefix.
/// Retain it separately from visible messages, together with explicit user goal
/// edits so a later pause or clear cannot disappear while older steering survives.
/// This is a retention decision only; visibility and authorization are unchanged.
pub(crate) fn retained_goal_context_text(item: &ResponseItem) -> Option<&str> {
    if let Some(text) = UserGoalUpdate::message_text(item) {
        return Some(text);
    }
    let ResponseItem::Message { role, content, .. } = item else {
        return None;
    };
    let [ContentItem::InputText { text }] = content.as_slice() else {
        return None;
    };
    let trimmed = text.trim();
    let is_goal = matches_legacy_goal_context(trimmed)
        || (trimmed.starts_with("<codex_internal_context source=\"goal\">")
            && InternalModelContextFragment::matches_text(trimmed));
    (role == "user" && is_goal).then_some(text)
}

fn is_valid_source(source: &str) -> bool {
    let mut chars = source.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}
