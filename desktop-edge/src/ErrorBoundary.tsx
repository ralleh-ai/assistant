import { Component, type ErrorInfo, type ReactNode } from "react";
import { assistantDiagnosticsBundle } from "./presence";

/**
 * Top-level React error boundary (finding F3).
 *
 * Without a boundary, any uncaught throw during render white-screens
 * the entire shell — the operator sees a blank window with no way to
 * recover short of quitting. This catches the throw, shows a
 * recoverable panel with the error detail, and offers two exits:
 * reload the webview (which re-runs the whole React tree from a clean
 * state, usually enough to recover from a transient render bug), or
 * capture a diagnostics bundle so the crash can be reported with the
 * same redacted context the settings panel produces.
 *
 * Error boundaries have no hook equivalent, so this stays a class
 * component — the one place in the frontend that must.
 */

type BoundaryState = {
  error: Error | null;
  /** Path of the diagnostics bundle once captured, or an error string. */
  diag: { phase: "idle" } | { phase: "capturing" } | { phase: "ready"; path: string } | { phase: "error"; message: string };
};

export class ErrorBoundary extends Component<{ children: ReactNode }, BoundaryState> {
  state: BoundaryState = { error: null, diag: { phase: "idle" } };

  static getDerivedStateFromError(error: Error): Partial<BoundaryState> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Surface the component stack to the devtools console; in a packaged
    // build this lands in the webview console, which the diagnostics
    // bundle's presence-log tail does not capture, so it's the one place
    // a developer can still see the full trace.
    console.error("Ralleh shell render error:", error, info.componentStack);
  }

  private handleReload = () => {
    window.location.reload();
  };

  private handleCaptureDiagnostics = async () => {
    this.setState({ diag: { phase: "capturing" } });
    try {
      const path = await assistantDiagnosticsBundle(null);
      this.setState({ diag: { phase: "ready", path } });
    } catch (err) {
      this.setState({ diag: { phase: "error", message: String(err) } });
    }
  };

  render() {
    if (!this.state.error) return this.props.children;

    const { diag } = this.state;
    return (
      <div className="shell-error-boundary" role="alert">
        <div className="shell-error-card">
          <h1 className="shell-error-title">Something went wrong</h1>
          <p className="shell-error-lede">
            The interface hit an unexpected error and stopped rendering. Your
            settings and audit log are safe on disk — reloading usually
            recovers.
          </p>
          <pre className="shell-error-detail">{this.state.error.message}</pre>
          <div className="shell-error-actions">
            <button
              type="button"
              className="backend-btn backend-btn-primary"
              onClick={this.handleReload}
            >
              Reload
            </button>
            <button
              type="button"
              className="backend-btn backend-btn-secondary"
              onClick={() => void this.handleCaptureDiagnostics()}
              disabled={diag.phase === "capturing"}
            >
              {diag.phase === "capturing" ? "Capturing…" : "Copy diagnostics"}
            </button>
          </div>
          {diag.phase === "ready" && (
            <p className="shell-error-diag" role="status">
              Diagnostics written to <code>{diag.path}</code>
            </p>
          )}
          {diag.phase === "error" && (
            <p className="shell-error-diag shell-error-diag-bad">
              Could not capture diagnostics: {diag.message}
            </p>
          )}
        </div>
      </div>
    );
  }
}
