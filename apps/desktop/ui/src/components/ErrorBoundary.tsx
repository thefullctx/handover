import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

/**
 * Last-resort boundary around the window root so an unexpected render error
 * shows a native-styled recovery screen instead of a blank translucent panel.
 */
export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Handover UI render error:", error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div
        className="status-body"
        role="alert"
        data-testid="error-boundary"
        style={{
          height: "100%",
          background: "var(--glass-fill)",
          borderRadius: "var(--r-window)",
        }}
      >
        <div className="status-icon error" aria-hidden="true">
          !
        </div>
        <h2>Something went wrong</h2>
        <p className="error-text">{this.state.error.message || "Unexpected interface error."}</p>
        <div className="status-actions">
          <button
            type="button"
            className="btn primary"
            onClick={() => window.location.reload()}
          >
            Reload
          </button>
        </div>
      </div>
    );
  }
}
