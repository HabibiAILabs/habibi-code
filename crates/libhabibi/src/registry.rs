//! Session-scoped tools shared by provider adapters and execution.
//!
//! Registering a tool is an explicit host action. Registrations are never loaded
//! from a repository or replayed from a session journal.
use crate::CancellationToken;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, future::Future, path::Path, pin::Pin, sync::Arc};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Structured execution value. Provider/journal rendering is deliberately separate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolOutput {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured: None,
            is_error: false,
        }
    }

    pub(crate) fn render(&self) -> String {
        match &self.structured {
            Some(value) => {
                serde_json::json!({"text": self.text, "structuredContent": value}).to_string()
            }
            None => self.text.clone(),
        }
    }
}

/// Context for an approved invocation. Implementations must bound their output
/// and honor cancellation, and must not return while a local mutation is still
/// running. The dispatcher does not drop arbitrary tool futures on cancellation.
pub struct ToolContext<'a> {
    pub workspace: &'a Path,
    pub cancel: CancellationToken,
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<ToolOutput>> + Send + 'a>>;

/// Trusted host extension, not a sandbox. All invocations require permission.
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    fn execute<'a>(&'a self, arguments: Value, context: ToolContext<'a>) -> ToolFuture<'a>;
}

struct Entry {
    definition: ToolDefinition,
    validator: jsonschema::Validator,
    handler: Option<Arc<dyn Tool>>,
}

pub(crate) struct ToolRegistry {
    entries: BTreeMap<String, Entry>,
}

impl ToolRegistry {
    pub fn new() -> Result<Self> {
        let mut registry = Self {
            entries: BTreeMap::new(),
        };
        for definition in crate::tools::definitions() {
            registry.insert(definition, None)?;
        }
        registry.insert(crate::codemode::definition(), None)?;
        Ok(registry)
    }

    pub(crate) fn insert(
        &mut self,
        definition: ToolDefinition,
        handler: Option<Arc<dyn Tool>>,
    ) -> Result<()> {
        let name = &definition.name;
        ensure!(
            handler.is_none() || name != "codemode",
            "codemode is a reserved tool name"
        );
        ensure!(
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
            "Tool names must contain 1–64 ASCII letters, digits, underscores or hyphens"
        );
        ensure!(
            !self.entries.contains_key(name),
            "Tool already registered: {name}"
        );
        ensure!(
            definition.schema.get("type").and_then(Value::as_str) == Some("object"),
            "Tool input schema must have type object"
        );
        // Remote/file resolution is disabled at compile time. Untrusted schemas
        // cannot cause implicit network requests or local file reads.
        let validator = jsonschema::validator_for(&definition.schema)
            .map_err(|e| anyhow::anyhow!("Invalid tool schema: {e}"))?;
        self.entries.insert(
            name.clone(),
            Entry {
                definition,
                validator,
                handler,
            },
        );
        Ok(())
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<()> {
        self.insert(tool.definition(), Some(tool))
    }

    pub fn register_many(&mut self, tools: Vec<Arc<dyn Tool>>) -> Result<()> {
        let mut pending = Self {
            entries: BTreeMap::new(),
        };
        for tool in tools {
            let definition = tool.definition();
            ensure!(
                !self.entries.contains_key(&definition.name),
                "Tool already registered: {}",
                definition.name
            );
            pending.insert(definition, Some(tool))?;
        }
        self.entries.extend(pending.entries);
        Ok(())
    }

    pub fn remove_mcp(&mut self) {
        self.entries.retain(|name, _| !name.starts_with("mcp__"));
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.entries
            .values()
            .map(|entry| entry.definition.clone())
            .collect()
    }

    pub fn validate(&self, name: &str, arguments: &Value) -> Result<()> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {name}"))?;
        entry
            .validator
            .validate(arguments)
            .map_err(|e| anyhow::anyhow!("Invalid {name} arguments: {e}"))
    }

    pub fn handler(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.entries
            .get(name)
            .and_then(|entry| entry.handler.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtin_validation_and_duplicate_protection() {
        let mut registry = ToolRegistry::new().unwrap();
        assert_eq!(registry.definitions().len(), 6);
        assert!(registry.validate("read", &json!({"path":"x"})).is_ok());
        assert!(
            registry
                .validate("read", &json!({"path":"x", "limit":0}))
                .is_err()
        );
        assert!(registry.validate("read", &json!({"path":7})).is_err());
        assert!(
            registry
                .validate("read", &json!({"path":"x", "unexpected":true}))
                .is_err()
        );
        assert!(registry.validate("unknown", &json!({})).is_err());
        let definition = registry.definitions().pop().unwrap();
        assert!(registry.insert(definition, None).is_err());
        assert_eq!(registry.definitions().len(), 6);
    }

    #[test]
    fn schemas_cannot_resolve_external_references() {
        for reference in ["file:///etc/passwd", "https://example.com/schema"] {
            let mut registry = ToolRegistry::new().unwrap();
            assert!(
                registry
                    .insert(
                        ToolDefinition {
                            name: "external".into(),
                            description: "test".into(),
                            schema: json!({"type":"object", "properties":{"x":{"$ref":reference}}}),
                        },
                        None
                    )
                    .is_err()
            );
        }
    }

    #[test]
    fn structured_results_are_not_silently_discarded() {
        let output = ToolOutput {
            text: "ok".into(),
            structured: Some(json!({"count":3})),
            is_error: false,
        };
        assert_eq!(
            serde_json::from_str::<Value>(&output.render()).unwrap()["structuredContent"]["count"],
            3
        );
    }
}
