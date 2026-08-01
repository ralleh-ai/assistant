//! ralleh-mcp-server
//!
//! Server-side Rust surface exposing `ralleh-tool-gateway` over HTTP, per
//! DEVELOPMENT.md's Rust-first architecture: the tool gateway, policy
//! engine, and this dispatch surface all live in Rust rather than the
//! TypeScript control plane, since this is exactly the
//! real-time/security-critical hot path the architecture reserves for Rust.
//!
//! This is intentionally a thin HTTP shell around `ToolGateway`. When
//! `RALLEH_API_TOKENS` / `RALLEH_API_TOKENS_FILE` is set, Bearer tokens bind
//! tenant/actor(/device) claims (threat model T1). Every tool call still
//! passes through the same policy-gated dispatch path already validated in
//! `ralleh-tool-gateway`.

pub mod auth;
pub mod config;
mod router;
mod state;

pub use auth::{AuthError, CallerIdentity, TokenAuthenticator};
pub use config::{resolve_config_path, ConfigError, HandlerKind, ServerConfig, ToolConfig};
pub use router::build_router;
pub use state::AppState;
