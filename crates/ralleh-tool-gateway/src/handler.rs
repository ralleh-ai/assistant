use serde::{Deserialize, Serialize};

/// A single invocation request handed to a `ToolHandler` after policy has
/// already allowed it. Handlers never see denied/unauthorized calls — by
/// the time this reaches a handler, the gateway has already confirmed
/// policy allowed it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub capability: String,
    pub tenant_id: String,
    pub device_id: String,
    pub actor_id: String,
    /// Structured arguments for the tool call. Handlers are responsible for
    /// their own argument validation — the gateway does not interpret this.
    pub arguments: serde_json::Value,
}

/// The result of executing a tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub summary: String,
    pub data: serde_json::Value,
}

/// Something capable of actually executing a specific capability once
/// policy has authorized it. Implementations wrap real integrations
/// (calendar, search, filesystem, external APIs, etc.); this crate ships no
/// concrete handlers itself, only the trait and the gateway that enforces
/// policy-gating around whatever handlers are registered.
pub trait ToolHandler: Send + Sync {
    fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, String>;
}

/// Simple in-memory handler useful for tests and for early integration
/// scaffolding: always returns a fixed result (or a fixed error), so gateway
/// dispatch behavior can be validated independent of any real integration.
pub struct EchoHandler;

impl ToolHandler for EchoHandler {
    fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, String> {
        Ok(ToolResult {
            summary: format!("echo:{}", invocation.capability),
            data: invocation.arguments.clone(),
        })
    }
}

/// Test-only handler that always fails, useful for validating the gateway
/// correctly distinguishes "policy denied" from "handler failed."
pub struct AlwaysFailHandler {
    pub error_message: String,
}

impl ToolHandler for AlwaysFailHandler {
    fn invoke(&self, _invocation: &ToolInvocation) -> Result<ToolResult, String> {
        Err(self.error_message.clone())
    }
}
