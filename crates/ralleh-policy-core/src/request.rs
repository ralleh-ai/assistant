use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A schema-validated request to perform a privileged action. Every field
/// here is required — there is no "ambient" scope. Callers must supply
/// tenant/device/user identity explicitly (see DEVELOPMENT.md §11.1:
/// "Tenant isolation must be enforced at the data-access layer, not only
/// application filters" — this struct makes it impossible to evaluate a
/// policy decision without that context present).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyRequest {
    /// Tenant the requester belongs to. Non-empty, required.
    pub tenant_id: String,
    /// Device the request originates from. Non-empty, required.
    pub device_id: String,
    /// User (or service identity) making the request. Non-empty, required.
    pub actor_id: String,
    /// The capability/tool/action being requested, e.g. "tool.github.create_issue".
    pub capability: String,
    /// Coarse data-sensitivity label for what this action would read/write,
    /// e.g. "public", "internal", "confidential", "regulated".
    pub sensitivity: String,
    /// Free-form context for logging/audit only. Never used in evaluation
    /// logic directly — evaluation must be driven by explicit fields above,
    /// not by parsing this bag.
    pub context: serde_json::Value,
}

/// Errors that make a request invalid *before* policy evaluation even runs.
/// These are distinct from a policy *denial* — a denial means "the request
/// was valid but not permitted"; a validation error means "this isn't even
/// a well-formed request."
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum RequestValidationError {
    #[error("tenant_id must not be empty")]
    EmptyTenantId,
    #[error("device_id must not be empty")]
    EmptyDeviceId,
    #[error("actor_id must not be empty")]
    EmptyActorId,
    #[error("capability must not be empty")]
    EmptyCapability,
    #[error("sensitivity must be one of: public, internal, confidential, regulated")]
    InvalidSensitivity,
}

impl PolicyRequest {
    pub fn new(
        tenant_id: impl Into<String>,
        device_id: impl Into<String>,
        actor_id: impl Into<String>,
        capability: impl Into<String>,
        sensitivity: impl Into<String>,
    ) -> Result<Self, RequestValidationError> {
        let req = PolicyRequest {
            tenant_id: tenant_id.into(),
            device_id: device_id.into(),
            actor_id: actor_id.into(),
            capability: capability.into(),
            sensitivity: sensitivity.into(),
            context: serde_json::Value::Null,
        };
        req.validate()?;
        Ok(req)
    }

    pub fn validate(&self) -> Result<(), RequestValidationError> {
        if self.tenant_id.trim().is_empty() {
            return Err(RequestValidationError::EmptyTenantId);
        }
        if self.device_id.trim().is_empty() {
            return Err(RequestValidationError::EmptyDeviceId);
        }
        if self.actor_id.trim().is_empty() {
            return Err(RequestValidationError::EmptyActorId);
        }
        if self.capability.trim().is_empty() {
            return Err(RequestValidationError::EmptyCapability);
        }
        match self.sensitivity.as_str() {
            "public" | "internal" | "confidential" | "regulated" => {}
            _ => return Err(RequestValidationError::InvalidSensitivity),
        }
        Ok(())
    }

    /// Stable id useful for correlating a request to its resulting decision
    /// in audit logs, independent of any storage layer.
    pub fn correlation_id(&self) -> Uuid {
        Uuid::new_v4()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_request_constructs_successfully() {
        let req = PolicyRequest::new("tenant-1", "device-1", "user-1", "tool.search", "public");
        assert!(req.is_ok());
    }

    #[test]
    fn empty_tenant_id_is_rejected() {
        let req = PolicyRequest::new("", "device-1", "user-1", "tool.search", "public");
        assert_eq!(req.unwrap_err(), RequestValidationError::EmptyTenantId);
    }

    #[test]
    fn empty_device_id_is_rejected() {
        let req = PolicyRequest::new("tenant-1", "", "user-1", "tool.search", "public");
        assert_eq!(req.unwrap_err(), RequestValidationError::EmptyDeviceId);
    }

    #[test]
    fn empty_actor_id_is_rejected() {
        let req = PolicyRequest::new("tenant-1", "device-1", "", "tool.search", "public");
        assert_eq!(req.unwrap_err(), RequestValidationError::EmptyActorId);
    }

    #[test]
    fn empty_capability_is_rejected() {
        let req = PolicyRequest::new("tenant-1", "device-1", "user-1", "", "public");
        assert_eq!(req.unwrap_err(), RequestValidationError::EmptyCapability);
    }

    #[test]
    fn invalid_sensitivity_is_rejected() {
        let req = PolicyRequest::new("tenant-1", "device-1", "user-1", "tool.search", "banana");
        assert_eq!(req.unwrap_err(), RequestValidationError::InvalidSensitivity);
    }

    #[test]
    fn whitespace_only_fields_are_rejected() {
        let req = PolicyRequest::new("   ", "device-1", "user-1", "tool.search", "public");
        assert_eq!(req.unwrap_err(), RequestValidationError::EmptyTenantId);
    }
}
