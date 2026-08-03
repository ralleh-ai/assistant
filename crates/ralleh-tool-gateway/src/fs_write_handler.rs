use std::fs;
use std::path::{Path, PathBuf};

use crate::handler::{ToolHandler, ToolInvocation, ToolResult};

/// Sibling of `FsReadTextHandler`: writes a UTF-8 text file within a
/// configured sandbox root. Same sandboxing contract -- policy decides
/// *whether* a tenant/actor may call `tool.fs.write_text` at all; this
/// handler still enforces its own root confinement regardless of what
/// policy already allowed, and refuses anything that would resolve outside
/// `root` even if the underlying OS call would otherwise succeed.
///
/// Deliberately conservative: no overwrite by default (mirrors
/// `file_write`'s own "refuse to clobber" default elsewhere in this
/// ecosystem) unless the caller explicitly passes `"overwrite": true`.
/// Parent directories are not created implicitly -- the target's parent
/// must already exist and already be inside the sandbox, which keeps this
/// handler from being usable to spray arbitrary directory trees into the
/// sandbox root without at least one prior legitimate write establishing
/// them.
pub struct FsWriteTextHandler {
    root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum FsWriteTextError {
    #[error("arguments must be a JSON object with a string \"path\" field")]
    MissingPathArgument,
    #[error("arguments must include a string \"contents\" field")]
    MissingContentsArgument,
    #[error("path escapes the configured sandbox root")]
    PathEscapesRoot,
    #[error("refusing to write through a symlink (sandbox escape guard)")]
    SymlinkRejected,
    #[error("refusing to overwrite existing file (pass \"overwrite\": true to allow)")]
    RefusingOverwrite,
    #[error("parent directory does not exist within the sandbox root")]
    ParentMissing,
    #[error("failed to write file: {0}")]
    Io(String),
}

impl FsWriteTextHandler {
    /// `root` must be an existing, canonicalizable directory. All writes
    /// are confined to this root; nothing above it, and nothing reached
    /// via symlink escape, is ever writable through this handler.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let root = root.into().canonicalize()?;
        Ok(Self { root })
    }

    /// Resolve `requested` against `root`, rejecting anything that would
    /// land outside it. Unlike the read handler, the target file need not
    /// already exist (that's the whole point of a write handler), so we
    /// canonicalize the *parent* directory (which must exist) and rebuild
    /// the full path from that, rather than canonicalizing the target
    /// itself.
    ///
    /// Canonicalizing the parent defeats `../` traversal in the directory
    /// components, but not a **symlink as the final path component** — a
    /// symlink named `link` inside the sandbox that points at `/etc/passwd`
    /// would otherwise be followed by `fs::write`. We therefore additionally
    /// reject a symlink leaf, and (when the target already exists)
    /// canonicalize the full path and re-confirm it stays under `root`. The
    /// write path itself uses `create_new` (fail-if-exists) so a symlink
    /// planted between this check and the open cannot be followed.
    fn resolve_within_root(&self, requested: &str) -> Result<PathBuf, FsWriteTextError> {
        let candidate = self.root.join(requested);

        let file_name = candidate
            .file_name()
            .ok_or(FsWriteTextError::PathEscapesRoot)?
            .to_owned();

        let parent = candidate
            .parent()
            .ok_or(FsWriteTextError::PathEscapesRoot)?;

        let canonical_parent = parent
            .canonicalize()
            .map_err(|_| FsWriteTextError::ParentMissing)?;

        if !canonical_parent.starts_with(&self.root) {
            return Err(FsWriteTextError::PathEscapesRoot);
        }

        let resolved = canonical_parent.join(&file_name);

        // If anything already exists at the leaf, it must not be a symlink,
        // and its real (symlink-resolved) location must stay under the root.
        if let Ok(meta) = fs::symlink_metadata(&resolved) {
            if meta.file_type().is_symlink() {
                return Err(FsWriteTextError::SymlinkRejected);
            }
            if let Ok(canonical_target) = resolved.canonicalize() {
                if !canonical_target.starts_with(&self.root) {
                    return Err(FsWriteTextError::PathEscapesRoot);
                }
            }
        }

        Ok(resolved)
    }
}

impl ToolHandler for FsWriteTextHandler {
    fn invoke(&self, invocation: &ToolInvocation) -> Result<ToolResult, String> {
        let requested_path = invocation
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or(FsWriteTextError::MissingPathArgument)
            .map_err(|e| e.to_string())?;

        let contents = invocation
            .arguments
            .get("contents")
            .and_then(|v| v.as_str())
            .ok_or(FsWriteTextError::MissingContentsArgument)
            .map_err(|e| e.to_string())?;

        let overwrite = invocation
            .arguments
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let resolved = self
            .resolve_within_root(requested_path)
            .map_err(|e| e.to_string())?;

        // `create_new` makes the "refuse to clobber" check atomic and, as a
        // side effect, refuses to follow a symlink planted at the leaf after
        // `resolve_within_root` ran (O_EXCL semantics). When overwrite is
        // allowed we truncate an existing regular file (symlink leaves were
        // already rejected during resolution).
        let mut open_opts = fs::OpenOptions::new();
        open_opts.write(true);
        if overwrite {
            open_opts.create(true).truncate(true);
        } else {
            open_opts.create_new(true);
        }

        let mut file = open_opts.open(&resolved).map_err(|e| {
            if !overwrite && e.kind() == std::io::ErrorKind::AlreadyExists {
                FsWriteTextError::RefusingOverwrite.to_string()
            } else {
                FsWriteTextError::Io(e.to_string()).to_string()
            }
        })?;
        std::io::Write::write_all(&mut file, contents.as_bytes())
            .map_err(|e| FsWriteTextError::Io(e.to_string()).to_string())?;

        Ok(ToolResult {
            summary: format!("wrote {} bytes to {}", contents.len(), requested_path),
            data: serde_json::json!({ "path": requested_path, "bytes_written": contents.len() }),
        })
    }
}

/// Exposed for tests/callers that want to confirm the sandbox root without
/// reaching into private state.
pub fn sandbox_root(handler: &FsWriteTextHandler) -> &Path {
    &handler.root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invocation(path: &str, contents: &str, overwrite: bool) -> ToolInvocation {
        ToolInvocation {
            capability: "tool.fs.write_text".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            arguments: serde_json::json!({
                "path": path,
                "contents": contents,
                "overwrite": overwrite,
            }),
        }
    }

    #[test]
    fn writes_a_new_file_within_the_sandbox_root() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();

        let result = handler
            .invoke(&invocation("hello.txt", "hello ralleh", false))
            .unwrap();

        assert_eq!(result.data["bytes_written"], 12);
        let on_disk = fs::read_to_string(dir.path().join("hello.txt")).unwrap();
        assert_eq!(on_disk, "hello ralleh");
    }

    #[test]
    fn refuses_to_overwrite_existing_file_by_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("existing.txt"), "original").unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();

        let err = handler
            .invoke(&invocation("existing.txt", "clobbered", false))
            .unwrap_err();
        assert_eq!(err, FsWriteTextError::RefusingOverwrite.to_string());

        // Original contents must be untouched.
        let on_disk = fs::read_to_string(dir.path().join("existing.txt")).unwrap();
        assert_eq!(on_disk, "original");
    }

    #[test]
    fn overwrite_true_allows_replacing_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("existing.txt"), "original").unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();

        handler
            .invoke(&invocation("existing.txt", "replaced", true))
            .unwrap();

        let on_disk = fs::read_to_string(dir.path().join("existing.txt")).unwrap();
        assert_eq!(on_disk, "replaced");
    }

    #[test]
    fn rejects_path_traversal_outside_root() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("sandboxed-root");
        fs::create_dir(&root).unwrap();
        let handler = FsWriteTextHandler::new(&root).unwrap();

        let err = handler
            .invoke(&invocation("../escape.txt", "malicious", false))
            .unwrap_err();
        assert_eq!(err, FsWriteTextError::PathEscapesRoot.to_string());

        // Nothing must have been written outside the sandbox.
        assert!(!parent.path().join("escape.txt").exists());
    }

    #[test]
    fn rejects_write_when_parent_directory_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();

        let err = handler
            .invoke(&invocation("nested/does-not-exist/file.txt", "x", false))
            .unwrap_err();
        assert_eq!(err, FsWriteTextError::ParentMissing.to_string());
    }

    #[test]
    fn missing_path_argument_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();

        let bad_invocation = ToolInvocation {
            capability: "tool.fs.write_text".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            arguments: serde_json::json!({ "contents": "x" }),
        };

        let err = handler.invoke(&bad_invocation).unwrap_err();
        assert_eq!(err, FsWriteTextError::MissingPathArgument.to_string());
    }

    #[test]
    fn missing_contents_argument_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();

        let bad_invocation = ToolInvocation {
            capability: "tool.fs.write_text".to_string(),
            tenant_id: "t1".to_string(),
            device_id: "d1".to_string(),
            actor_id: "u1".to_string(),
            arguments: serde_json::json!({ "path": "x.txt" }),
        };

        let err = handler.invoke(&bad_invocation).unwrap_err();
        assert_eq!(err, FsWriteTextError::MissingContentsArgument.to_string());
    }

    #[test]
    fn sandbox_root_returns_configured_root() {
        let dir = tempfile::tempdir().unwrap();
        let handler = FsWriteTextHandler::new(dir.path()).unwrap();
        assert_eq!(sandbox_root(&handler), dir.path().canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_write_through_a_symlink_leaf_escaping_the_sandbox() {
        use std::os::unix::fs::symlink;

        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("secret.txt");
        fs::write(&target, "original outside contents").unwrap();

        let sandbox = tempfile::tempdir().unwrap();
        // Plant a symlink INSIDE the sandbox whose leaf points outside it.
        symlink(&target, sandbox.path().join("escape.txt")).unwrap();

        let handler = FsWriteTextHandler::new(sandbox.path()).unwrap();

        // Both overwrite and non-overwrite must refuse to follow the symlink.
        let err = handler
            .invoke(&invocation("escape.txt", "attacker", true))
            .unwrap_err();
        assert_eq!(err, FsWriteTextError::SymlinkRejected.to_string());

        // The external file must be untouched.
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "original outside contents"
        );
    }
}
