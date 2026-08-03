//! ralleh-policy-core
//!
//! Enterprise policy evaluation engine for Ralleh. This crate is the single
//! authority for deciding whether a privileged action (tool call, capability
//! use, data access) is allowed.
//!
//! Design invariants (see projects/voice-assistant/DEVELOPMENT.md §11, §20 ADR-004):
//! - Deny by default. An action is only allowed if an explicit rule permits it.
//! - Every decision is schema-validated (no free-form/ad-hoc requests).
//! - Every decision produces an auditable `PolicyDecision` record — callers
//!   are expected to persist this via an audit sink; this crate does not
//!   perform I/O itself (keeps it pure, fast, and trivially testable).
//! - Tenant/device/user scoping is a first-class part of every request, not
//!   an afterthought bolted on by callers.

mod decision;
mod egress;
mod engine;
mod request;
mod rule;

pub use decision::{PolicyDecision, PolicyOutcome};
pub use egress::{
    EgressDenialReason, EgressDenied, EgressPolicy, ALLOWED_HOSTS_ENV, DEFAULT_ALLOWED_HOSTS,
};
pub use engine::PolicyEngine;
pub use request::PolicyRequest;
pub use rule::{PolicyRule, RuleEffect};
