//! Declarative server configuration: tool registry + policy rules loaded
//! from a TOML or JSON file instead of being hardcoded in `main`.
//!
//! This closes the biggest spine gap called out in `docs/NEXT_STEPS.md`:
//! DEVELOPMENT.md §8.3 requires declarative policy rules compiled/loaded
//! by the Rust evaluator. Handler *implementations* stay in code (they
//! can't be deserialized); the config only names which known handlers to
//! wire up and under which capability ids.

use std::fs;
use std::path::{Path, PathBuf};

use ralleh_policy_core::{PolicyEngine, PolicyRule};
use ralleh_tool_gateway::{
    FsReadTextHandler, FsWriteTextHandler, HttpFetchHandler, ToolDefinition, ToolRegistry,
};
use serde::Deserialize;

/// Top-level server config file shape.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Directory all `fs_*` handlers are sandboxed to. If omitted, the
    /// loader picks a temp-dir default (matching prior hardcoded behavior).
    #[serde(default)]
    pub sandbox_root: Option<PathBuf>,

    /// Tools to register. Each entry names a known handler implementation.
    #[serde(default)]
    pub tools: Vec<ToolConfig>,

    /// Ordered policy rules (first match wins — same semantics as
    /// `PolicyEngine`). Empty means deny-by-default for everything.
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

/// One tool registration entry.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolConfig {
    pub capability: String,
    pub description: String,
    pub default_sensitivity: String,
    /// Which built-in handler implementation to wire up.
    pub handler: HandlerKind,
    /// Egress allowlist for `http_fetch` (exact hostnames). Ignored by
    /// filesystem handlers. Required non-empty when `handler = http_fetch`.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

/// Known handler implementations the server knows how to construct.
///
/// Adding a new real handler means: implement it in `ralleh-tool-gateway`
/// (or elsewhere), add a variant here, and teach `build_registry` how to
/// construct it. Config files then opt in by name — no code change in
/// `main` required beyond the variant.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandlerKind {
    FsReadText,
    FsWriteText,
    HttpFetch,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("unsupported config format for {path} (expected .toml or .json)")]
    UnsupportedFormat { path: PathBuf },
    #[error("failed to initialize handler for capability '{capability}': {message}")]
    HandlerInit {
        capability: String,
        message: String,
    },
    #[error("config validation failed: {0}")]
    Validation(String),
}

impl ServerConfig {
    /// Load from a path, choosing the deserializer from the file extension
    /// (`.toml` or `.json`). Matches DEVELOPMENT.md §8.3's "YAML/JSON"
    /// intent; TOML is the preferred format for this Rust workspace, JSON
    /// is supported because serde_json is already a workspace dependency
    /// and YAML would add another crate for little gain right now.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        let config: ServerConfig = match ext.as_str() {
            "toml" => toml::from_str(&contents).map_err(|e| ConfigError::Parse {
                path: path.to_path_buf(),
                source: Box::new(e),
            })?,
            "json" => serde_json::from_str(&contents).map_err(|e| ConfigError::Parse {
                path: path.to_path_buf(),
                source: Box::new(e),
            })?,
            _ => {
                return Err(ConfigError::UnsupportedFormat {
                    path: path.to_path_buf(),
                })
            }
        };

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for tool in &self.tools {
            if tool.capability.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "tool capability must not be empty".into(),
                ));
            }
            if !seen.insert(tool.capability.clone()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate tool capability '{}'",
                    tool.capability
                )));
            }
            if tool.handler == HandlerKind::HttpFetch && tool.allowed_hosts.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "tool '{}' (http_fetch) requires a non-empty allowed_hosts egress allowlist",
                    tool.capability
                )));
            }
        }
        for rule in &self.rules {
            if rule.id.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "policy rule id must not be empty".into(),
                ));
            }
            if rule.reason.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "policy rule '{}' must have a non-empty reason",
                    rule.id
                )));
            }
        }
        Ok(())
    }

    /// Resolve the sandbox directory: explicit config value, else a
    /// temp-dir default. Creates the directory if needed.
    pub fn resolve_sandbox_root(&self) -> Result<PathBuf, ConfigError> {
        let root = self.sandbox_root.clone().unwrap_or_else(|| {
            std::env::temp_dir().join("ralleh-mcp-server-sandbox")
        });
        fs::create_dir_all(&root).map_err(|source| ConfigError::Io {
            path: root.clone(),
            source,
        })?;
        Ok(root)
    }

    /// Build a `ToolRegistry` by constructing the named handlers against
    /// `sandbox_root`.
    pub fn build_registry(&self, sandbox_root: &Path) -> Result<ToolRegistry, ConfigError> {
        let mut registry = ToolRegistry::new();
        for tool in &self.tools {
            let definition = ToolDefinition {
                capability: tool.capability.clone(),
                description: tool.description.clone(),
                default_sensitivity: tool.default_sensitivity.clone(),
            };
            let handler: Box<dyn ralleh_tool_gateway::ToolHandler> = match tool.handler {
                HandlerKind::FsReadText => Box::new(
                    FsReadTextHandler::new(sandbox_root).map_err(|source| {
                        ConfigError::HandlerInit {
                            capability: tool.capability.clone(),
                            message: source.to_string(),
                        }
                    })?,
                ),
                HandlerKind::FsWriteText => Box::new(
                    FsWriteTextHandler::new(sandbox_root).map_err(|source| {
                        ConfigError::HandlerInit {
                            capability: tool.capability.clone(),
                            message: source.to_string(),
                        }
                    })?,
                ),
                HandlerKind::HttpFetch => Box::new(
                    HttpFetchHandler::new(tool.allowed_hosts.clone()).map_err(|source| {
                        ConfigError::HandlerInit {
                            capability: tool.capability.clone(),
                            message: source.to_string(),
                        }
                    })?,
                ),
            };
            registry.register(definition, handler);
        }
        Ok(registry)
    }

    /// Build a `PolicyEngine` from the ordered rule list.
    pub fn build_policy_engine(&self) -> PolicyEngine {
        PolicyEngine::new(self.rules.clone())
    }
}

/// Resolve which config file to load.
///
/// Precedence:
/// 1. `RALLEH_CONFIG` env var (explicit path)
/// 2. `config/default.toml` relative to the current working directory
pub fn resolve_config_path() -> Result<PathBuf, ConfigError> {
    if let Ok(explicit) = std::env::var("RALLEH_CONFIG") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        return Err(ConfigError::Io {
            path,
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "RALLEH_CONFIG points to a missing file",
            ),
        });
    }

    let default = PathBuf::from("config/default.toml");
    if default.is_file() {
        return Ok(default);
    }

    Err(ConfigError::Io {
        path: default,
        source: std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no RALLEH_CONFIG set and config/default.toml not found in cwd; \
             set RALLEH_CONFIG or run from the repo root",
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralleh_policy_core::{PolicyRequest, RuleEffect};
    use std::io::Write;

    fn sample_toml() -> &'static str {
        r#"
sandbox_root = "PLACEHOLDER"

[[tools]]
capability = "tool.fs.read_text"
description = "Read a UTF-8 text file from the sandboxed directory."
default_sensitivity = "internal"
handler = "fs_read_text"

[[tools]]
capability = "tool.fs.write_text"
description = "Write a UTF-8 text file into the sandboxed directory."
default_sensitivity = "internal"
handler = "fs_write_text"

[[rules]]
id = "default-allow-fs-read"
capability_prefix = "tool.fs.read_text"
effect = "Allow"
reason = "sandboxed fs reads are allowed"

[[rules]]
id = "default-approval-fs-write"
capability_prefix = "tool.fs.write_text"
effect = "RequireApproval"
reason = "sandboxed fs writes require approval"
"#
    }

    #[test]
    fn loads_toml_and_builds_registry_and_policy() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        fs::create_dir_all(&sandbox).unwrap();

        let config_path = dir.path().join("server.toml");
        let body = sample_toml().replace("PLACEHOLDER", &sandbox.display().to_string().replace('\\', "/"));
        fs::write(&config_path, body).unwrap();

        let config = ServerConfig::load_from_path(&config_path).unwrap();
        assert_eq!(config.tools.len(), 2);
        assert_eq!(config.rules.len(), 2);
        assert_eq!(config.tools[0].handler, HandlerKind::FsReadText);
        assert_eq!(config.tools[1].handler, HandlerKind::FsWriteText);

        let registry = config.build_registry(&sandbox).unwrap();
        assert!(registry.is_registered("tool.fs.read_text"));
        assert!(registry.is_registered("tool.fs.write_text"));

        let engine = config.build_policy_engine();
        let read_req = PolicyRequest::new(
            "t1",
            "d1",
            "a1",
            "tool.fs.read_text",
            "internal",
        )
        .unwrap();
        let write_req = PolicyRequest::new(
            "t1",
            "d1",
            "a1",
            "tool.fs.write_text",
            "internal",
        )
        .unwrap();

        assert_eq!(
            engine.evaluate(&read_req).outcome,
            ralleh_policy_core::PolicyOutcome::Allowed
        );
        assert_eq!(
            engine.evaluate(&write_req).outcome,
            ralleh_policy_core::PolicyOutcome::ApprovalRequired
        );
        // Prove the deserialized effect variants round-trip as expected.
        assert_eq!(config.rules[0].effect, RuleEffect::Allow);
        assert_eq!(config.rules[1].effect, RuleEffect::RequireApproval);
    }

    #[test]
    fn loads_json_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.json");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            r#"{{
              "tools": [{{
                "capability": "tool.fs.read_text",
                "description": "read",
                "default_sensitivity": "internal",
                "handler": "fs_read_text"
              }}],
              "rules": [{{
                "id": "allow-read",
                "capability_prefix": "tool.fs.read_text",
                "effect": "Allow",
                "reason": "ok"
              }}]
            }}"#
        )
        .unwrap();

        let config = ServerConfig::load_from_path(&path).unwrap();
        assert_eq!(config.tools.len(), 1);
        assert_eq!(config.rules.len(), 1);
    }

    #[test]
    fn rejects_duplicate_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        fs::write(
            &path,
            r#"
[[tools]]
capability = "tool.fs.read_text"
description = "a"
default_sensitivity = "internal"
handler = "fs_read_text"

[[tools]]
capability = "tool.fs.read_text"
description = "b"
default_sensitivity = "internal"
handler = "fs_read_text"
"#,
        )
        .unwrap();

        let err = ServerConfig::load_from_path(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn rejects_empty_rule_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        fs::write(
            &path,
            r#"
[[rules]]
id = "r1"
effect = "Allow"
reason = ""
"#,
        )
        .unwrap();

        let err = ServerConfig::load_from_path(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn rejects_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.yaml");
        fs::write(&path, "tools: []\n").unwrap();
        let err = ServerConfig::load_from_path(&path).unwrap_err();
        assert!(matches!(err, ConfigError::UnsupportedFormat { .. }));
    }

    #[test]
    fn rejects_http_fetch_without_allowed_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        fs::write(
            &path,
            r#"
[[tools]]
capability = "tool.http.fetch"
description = "fetch"
default_sensitivity = "internal"
handler = "http_fetch"
"#,
        )
        .unwrap();
        let err = ServerConfig::load_from_path(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Validation(_)));
    }

    #[test]
    fn loads_http_fetch_with_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let sandbox = dir.path().join("sandbox");
        fs::create_dir_all(&sandbox).unwrap();
        let path = dir.path().join("server.toml");
        fs::write(
            &path,
            format!(
                r#"
sandbox_root = "{}"

[[tools]]
capability = "tool.http.fetch"
description = "fetch"
default_sensitivity = "internal"
handler = "http_fetch"
allowed_hosts = ["example.com"]

[[rules]]
id = "allow-fetch"
capability_prefix = "tool.http.fetch"
effect = "Allow"
reason = "ok"
"#,
                sandbox.display().to_string().replace('\\', "/")
            ),
        )
        .unwrap();

        let config = ServerConfig::load_from_path(&path).unwrap();
        assert_eq!(config.tools[0].handler, HandlerKind::HttpFetch);
        assert_eq!(config.tools[0].allowed_hosts, vec!["example.com"]);
        let registry = config.build_registry(&sandbox).unwrap();
        assert!(registry.is_registered("tool.http.fetch"));
    }
}
