//! ralleh-tool-gateway
//!
//! The single chokepoint every tool/capability call must pass through.
//! Nothing in Ralleh invokes an external tool, integration, or privileged
//! capability directly — it goes through this gateway, which:
//!   1. Looks up the requested capability in a registry (unknown
//!      capabilities are rejected before policy is even consulted).
//!   2. Builds a `ralleh_policy_core::PolicyRequest` and evaluates it
//!      against the shared `PolicyEngine`.
//!   3. Only on `Allowed` does it invoke the registered `ToolHandler`.
//!   4. Every call — allowed, denied, or approval-required — produces an
//!      audit-ready `GatewayEvent`, independent of whether the underlying
//!      tool call itself succeeds or fails.
//!
//! This directly implements DEVELOPMENT.md's non-negotiable: "policy and
//! authorization logic must never live outside the shared Rust policy
//! core" — this crate does not reimplement policy logic; it only enforces
//! that every dispatch is *routed through* `ralleh-policy-core` and refuses
//! to run anything the policy engine didn't explicitly allow.

mod approval;
mod event;
mod fs_read_handler;
mod fs_write_handler;
mod http_fetch_handler;
pub mod gateway;
mod handler;
mod registry;

pub use approval::{ApprovalError, ApprovalRequest, ApprovalStatus, ApprovalStore};
pub use event::{GatewayEvent, ToolCallOutcome};
pub use fs_read_handler::{sandbox_root as fs_read_sandbox_root, FsReadTextError, FsReadTextHandler};
pub use fs_write_handler::{sandbox_root as fs_write_sandbox_root, FsWriteTextError, FsWriteTextHandler};
pub use http_fetch_handler::{HttpFetchError, HttpFetchHandler};
pub use gateway::{AuditSink, ToolGateway};
pub use handler::{AlwaysFailHandler, EchoHandler, ToolHandler, ToolInvocation, ToolResult};
pub use registry::{ToolDefinition, ToolRegistry};
