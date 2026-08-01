//! ralleh-ai-router
//!
//! Routes model/completion requests to a pluggable AI backend. This is the
//! third Rust hot-path service alongside `ralleh-policy-core` and
//! `ralleh-tool-gateway`, per DEVELOPMENT.md's Rust-first architecture:
//! request routing to LLM providers is latency-sensitive and benefits from
//! the same reliability discipline as tool dispatch.
//!
//! Design mirrors `ralleh-tool-gateway` deliberately:
//!   - A trait (`CompletionBackend`) abstracts over concrete providers,
//!     the same way `ToolHandler` abstracts over concrete tools. No
//!     provider-specific logic lives in the router itself.
//!   - Every request produces a `RoutingDecision` + outcome record,
//!     mirroring `GatewayEvent`, so routing is auditable the same way tool
//!     dispatch is.
//!   - Backend selection is policy-free by design *within this crate*, but
//!     built with the same seam `ralleh-policy-core` could later gate
//!     through (e.g. "tenant X may only route to backend Y") without
//!     reworking the router's shape.

mod backend;
mod request;
mod router;

pub use backend::{CompletionBackend, EchoBackend, HttpCompletionBackend};
pub use request::{CompletionOutcome, CompletionRequest, CompletionResponse};
pub use router::{AiRouter, RoutingError};
