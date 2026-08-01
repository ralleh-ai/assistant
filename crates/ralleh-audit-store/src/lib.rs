//! ralleh-audit-store
//!
//! Persists the audit-ready records produced elsewhere in the workspace —
//! `ralleh_tool_gateway::GatewayEvent` and AI-router completion outcomes —
//! to a durable sink. Per DEVELOPMENT.md's non-negotiable ("audit events
//! for every privileged action"), those crates only *construct* the
//! records; this crate is where they actually get written down so they
//! survive process restarts and can be queried later.
//!
//! Design mirrors the rest of the workspace's trait-boundary discipline:
//! `AuditSink` is the seam. `JsonlFileAuditSink` is the one real (not
//! mocked) implementation shipped today — an append-only, one-JSON-object-
//! per-line file, chosen deliberately over a database dependency because:
//!   - It requires no schema/migrations to get real persistence working.
//!   - Append-only writes are cheap and don't need a running DB process,
//!     which matters on this resource-constrained dev host.
//!   - It's trivially auditable/greppable and easy to ship into a real
//!     datastore later without changing any call site — every caller only
//!     depends on the `AuditSink` trait, never the file format directly.
//!
//! A `NullAuditSink` is provided for tests that don't care about
//! persistence, and an `InMemoryAuditSink` for tests that want to assert
//! on exactly what was recorded without touching the filesystem.

mod record;
mod sink;

pub use record::{AuditRecord, AuditRecordKind};
pub use sink::{AuditSink, AuditSinkError, InMemoryAuditSink, JsonlFileAuditSink, NullAuditSink};
