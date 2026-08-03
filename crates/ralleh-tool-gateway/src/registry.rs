use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::handler::ToolHandler;

/// Static metadata about a registered capability. Used to reject unknown
/// capabilities before policy is even consulted, and to give operators a
/// single place to see everything the gateway is capable of dispatching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub capability: String,
    pub description: String,
    /// Default sensitivity label used when a caller doesn't specify one
    /// explicitly for this capability's invocation.
    pub default_sensitivity: String,
}

/// Holds registered tool definitions and their handlers. Deliberately
/// separate from `ToolGateway` so the registry (what tools exist) and the
/// gateway (how calls are dispatched/audited) have a clean single
/// responsibility split — useful both for testing in isolation and for a
/// future admin surface that just needs to list `ToolDefinition`s.
pub struct ToolRegistry {
    definitions: HashMap<String, ToolDefinition>,
    handlers: HashMap<String, Box<dyn ToolHandler>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            definitions: HashMap::new(),
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, definition: ToolDefinition, handler: Box<dyn ToolHandler>) {
        let capability = definition.capability.clone();
        self.definitions.insert(capability.clone(), definition);
        self.handlers.insert(capability, handler);
    }

    pub fn definition(&self, capability: &str) -> Option<&ToolDefinition> {
        self.definitions.get(capability)
    }

    pub fn handler(&self, capability: &str) -> Option<&dyn ToolHandler> {
        self.handlers.get(capability).map(|h| h.as_ref())
    }

    pub fn is_registered(&self, capability: &str) -> bool {
        self.definitions.contains_key(capability)
    }

    pub fn capabilities(&self) -> Vec<&str> {
        self.definitions.keys().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::EchoHandler;

    #[test]
    fn unregistered_capability_is_not_found() {
        let registry = ToolRegistry::new();
        assert!(!registry.is_registered("tool.search"));
        assert!(registry.definition("tool.search").is_none());
        assert!(registry.handler("tool.search").is_none());
    }

    #[test]
    fn registered_capability_is_found_with_matching_definition() {
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.search".to_string(),
                description: "web search".to_string(),
                default_sensitivity: "public".to_string(),
            },
            Box::new(EchoHandler),
        );

        assert!(registry.is_registered("tool.search"));
        assert_eq!(
            registry.definition("tool.search").unwrap().description,
            "web search"
        );
        assert!(registry.handler("tool.search").is_some());
    }

    #[test]
    fn capabilities_lists_all_registered_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(
            ToolDefinition {
                capability: "tool.search".to_string(),
                description: "web search".to_string(),
                default_sensitivity: "public".to_string(),
            },
            Box::new(EchoHandler),
        );
        registry.register(
            ToolDefinition {
                capability: "tool.calendar".to_string(),
                description: "calendar access".to_string(),
                default_sensitivity: "internal".to_string(),
            },
            Box::new(EchoHandler),
        );

        let mut caps = registry.capabilities();
        caps.sort();
        assert_eq!(caps, vec!["tool.calendar", "tool.search"]);
    }
}
