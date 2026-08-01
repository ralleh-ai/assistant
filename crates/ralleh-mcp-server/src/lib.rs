//! ralleh-mcp-server
//!
//! Server-side Rust surface exposing `ralleh-tool-gateway` over HTTP, per
//! DEVELOPMENT.md's Rust-first architecture: the tool gateway, policy
//! engine, and this dispatch surface all live in Rust rather than the
//! TypeScript control plane, since this is exactly the
//! real-time/security-critical hot path the architecture reserves for Rust.
//!
//! This is intentionally a thin HTTP shell around `ToolGateway`. It adds no
//! authorization logic of its own -- every request still passes through
//! the same policy-gated dispatch path already validated in
//! `ralleh-tool-gateway`. The server's only responsibilities are: parse the
//! request, call `dispatch`, translate the resulting `GatewayEvent` into an
//! HTTP response and status code, and (later) push the event to durable
//! audit storage.

pub mod config;
mod router;
mod state;

pub use config::{resolve_config_path, ConfigError, HandlerKind, ServerConfig, ToolConfig};
pub use router::build_router;
pub use state::AppState;
