use std::fs;
use std::path::{Path, PathBuf};

use crate::handler::{ToolHandler, ToolInvocation, ToolResult};

/// A real (non-mocked) tool handler: reads a UTF-8 text file from within a
/// configured root directory and returns its contents. This is the first
/// handler in the gateway that isn't a test double — it performs a real
/// filesystem operation, gated end-to-end by the policy engine via
/// `ToolGateway::dispatch`.
///
/// Deliberately narrow and defensive: this is meant to prove the gateway's
/// real-world dispatch path works, not to be a general-purpose filesystem
/// tool. Path traversal outside `root` is rejected unconditionally,
/// independent of whatever policy already allowed the capability call --
/// policy decides *whether* a tenant/actor may call `tool.fs.read_text` at
/// all; this handler still enforces its own sandbox regardless.
pub struct FsReadTextHandler {
    root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum FsReadTextError {
    #[error("arguments must be a JSON object with a string \"path\" field")]
    MissingPathArgument,
    #[error("path escapes the configured sandbox root")]
    PathEscapesRoot,
    #[error("failed to read file: {0}")]
    Io(String),
    #[error("file contents are not valid UTF-8")]
    NotUtf8,
}

impl FsReadTextHandler {
    /// `root` must be an existing, canonicalizable directory. All reads are
    /// confined to this root; nothing above it is ever reachable through
    /// this handler no matter what arguments are supplied.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let root = root.into().canonicalize()?;
        Ok(Self { root })
    }

    fn resolve_within_root(&self, requested: &str) -> Result<PathBuf, FsReadTextError> {
        let candidate = self.root.join(requested);

        // Canonicalize to resolve `..`/symlinks, then verify the result is
        // still inside root. This is the actual sandbox enforcement --
        // string-prefix checks on non-canonical paths are not sufficient
        // (e.g. `../` sequences or symlinks could otherwise escape).
        let canonical = candidate
            .canonicalize()
            .map_err(|e| FsReadTextError::Io(e.to_string()))?;

        if !canonical.starts_with(&self.root) {
            return Err(FsReadTextError::PathEscapesRoot);
        }

        Ok(canonical)
    }
}

impl ToolHandler for FsReadTextHandler {
    fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, String> {
        let requested_path = invocation
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or(FsReadTextError::MissingPathArgument)
            .map_err(|e| e.to_string())?;

        let resolved = self
            .resolve_within_root(requested_path)
            .map_err(|e| e.to_string())?;

        let bytes = fs::read(&resolved).map_err(|e| FsReadTextError::Io(e.to_string()).to_string())?;
        let contents = String::from_utf8(bytes).map_err(|_| FsReadTextError::NotUtf8.to_string())?;

        Ok(ToolResult {
            summary: format!("read {} bytes from {}", contents.len(), requested_path),
            data: serde_json::json!({ "contents": contents }),
        })
    }
}

/// Exposed for tests/callers that want to confirm the sandbox root without
/// reaching into private state.
pub fn sandbox_root(handler: &FsReadTextHandler) -> &Path {
    &handler.root
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    fn invocation(path: &str) -> ToolInvocation {
        ToolInvocation {
            capability: "tool.fs.read_text".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            arguments: serde_json::json!({ "path": path }),
        }
    }

    #[test]
    fn reads_a_file_within_the_sandbox_root() {
        let dir = tempfile::tempdir().unwrap();
        write_temp_file(dir.path(), "hello.txt", "hello ralleh");

        let handler = FsReadTextHandler::new(dir.path()).unwrap();
        let result = handler.invoke(&invocation("hello.txt")).unwrap();

        assert_eq!(result.data["contents"], "hello ralleh");
        assert!(result.summary.contains("hello.txt"));
    }

    #[test]
    fn rejects_path_traversal_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        // A sibling file outside the sandboxed root.
        let outside_dir = tempfile::tempdir().unwrap();
        write_temp_file(outside_dir.path(), "secret.txt", "should not be readable");

        let handler = FsReadTextHandler::new(dir.path()).unwrap();

        // Attempt to escape via `..` back out to the outside dir. Since the
        // outside dir isn't necessarily a direct parent, this specific
        // traversal string may not resolve to a real file -- what matters
        // is that any resolution outside `root` is rejected, which we
        // verify with a path that provably escapes (parent of root).
        let escape_attempt = invocation("../escape-marker-that-should-not-resolve");
        let err = handler.invoke(&escape_attempt).unwrap_err();
        assert!(err.contains("read file") || err.contains("escapes"));
    }

    #[test]
    fn rejects_traversal_that_successfully_resolves_outside_root() {
        // Construct a root that has a real sibling file one level up, and
        // confirm `..`-based traversal to it is rejected even though the
        // file genuinely exists and is readable by the OS.
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("sandboxed-root");
        fs::create_dir(&root).unwrap();
        write_temp_file(parent.path(), "sibling-secret.txt", "top secret");

        let handler = FsReadTextHandler::new(&root).unwrap();
        let err = handler
            .invoke(&invocation("../sibling-secret.txt"))
            .unwrap_err();
        assert_eq!(err, FsReadTextError::PathEscapesRoot.to_string());
    }

    #[test]
    fn missing_path_argument_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsReadTextHandler::new(dir.path()).unwrap();

        let bad_invocation = ToolInvocation {
            capability: "tool.fs.read_text".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            arguments: serde_json::json!({}),
        };

        let err = handler.invoke(&bad_invocation).unwrap_err();
        assert_eq!(err, FsReadTextError::MissingPathArgument.to_string());
    }

    #[test]
    fn nonexistent_file_returns_io_error_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsReadTextHandler::new(dir.path()).unwrap();
        let err = handler.invoke(&invocation("does-not-exist.txt")).unwrap_err();
        assert!(err.contains("read file"));
    }

    #[test]
    fn invalid_utf8_contents_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binary.dat");
        fs::write(&path, [0xFF, 0xFE, 0x00, 0xFF]).unwrap();

        let handler = FsReadTextHandler::new(dir.path()).unwrap();
        let err = handler.invoke(&invocation("binary.dat")).unwrap_err();
        assert_eq!(err, FsReadTextError::NotUtf8.to_string());
    }
}
