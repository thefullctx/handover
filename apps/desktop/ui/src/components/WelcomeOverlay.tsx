import { useEffect, useRef } from "react";
import { AgentLogo, HandoverMark, Icon } from "../Icons";
import { formatShortcut } from "../lib/format";
import type { GalleryAgent } from "../lib/types";

interface Props {
  /** The configured accelerator, e.g. `CmdOrCtrl+Shift+A`. */
  shortcut: string;
  /** Detected-but-not-added catalog agents (installed ones first). */
  gallery: GalleryAgent[];
  /** One-click add from the welcome screen (no navigation). */
  onAddAgent: (a: GalleryAgent) => void;
  onDismiss: () => void;
}

/**
 * First-run onboarding — Apple "What's New"-style sheet with one-tap agent
 * additions and an icon-left feature overview.
 */
export default function WelcomeOverlay({
  shortcut,
  gallery,
  onAddAgent,
  onDismiss,
}: Props) {
  const glyph = formatShortcut(shortcut);
  const startRef = useRef<HTMLButtonElement>(null);

  const candidates = [...gallery]
    .filter((a) => !a.configured)
    .sort((a, b) => Number(!!b.detected_path) - Number(!!a.detected_path));
  const showPicker = candidates.length > 0;

  useEffect(() => {
    startRef.current?.focus();
  }, []);

  const onOverlayKey = (e: React.KeyboardEvent) => {
    if (e.key === "Tab") {
      e.preventDefault();
      startRef.current?.focus();
    }
  };

  return (
    <div
      className="welcome-overlay"
      role="dialog"
      aria-modal="true"
      aria-label="Welcome to Handover"
      data-testid="welcome"
      onKeyDown={onOverlayKey}
    >
      <div className="welcome-card">
        <span className="welcome-mark">
          <HandoverMark size={52} tile />
        </span>
        <h2>Welcome to Handover</h2>
        <p className="welcome-lead">
          Press <kbd>{glyph}</kbd> anywhere to pick a local agent and keep the
          conversation going.
        </p>

        {showPicker && (
          <>
            <div className="welcome-pickers" data-testid="welcome-pickers">
              <div className="welcome-picker-title">Detected Assistants</div>
              <div className="welcome-picker-group">
                {candidates.map((a) => (
                  <button
                    key={a.id}
                    type="button"
                    className={`welcome-agent${a.detected_path ? " detected" : ""}`}
                    onClick={() => onAddAgent(a)}
                    disabled={!a.detected_path}
                    aria-label={
                      a.detected_path ? `Add ${a.name}` : `${a.name} — not found`
                    }
                    title={
                      a.detected_path
                        ? `Add ${a.name} — it is already installed on this Mac`
                        : `${a.name} was not found on this Mac — add it from Settings after installing`
                    }
                  >
                    <span className="welcome-agent-logo">
                      <AgentLogo id={a.id} size={17} />
                    </span>
                    <span className="welcome-agent-name">{a.name}</span>
                    {a.signed_in === true && (
                      <span className="chip ok">Signed in</span>
                    )}
                    {a.detected_path ? (
                      <span className="welcome-agent-add">
                        <Icon name="plus" size={11} />
                        Add
                      </span>
                    ) : (
                      <span className="chip off">Not found</span>
                    )}
                  </button>
                ))}
              </div>
            </div>
            <p className="welcome-hint">
              Added agents appear right away. More options live in Settings → Agents.
            </p>
          </>
        )}

        <div className="welcome-features" aria-hidden="true">
          <div className="welcome-feature">
            <span className="welcome-feature-icon">
              <Icon name="keyboard" size={16} />
            </span>
            <div className="welcome-feature-text">
              <strong>Press {glyph} anywhere</strong>
              <span>Opens directly to your unfolded agent list.</span>
            </div>
          </div>
          <div className="welcome-feature">
            <span className="welcome-feature-icon">
              <Icon name="agentLocal" size={16} />
            </span>
            <div className="welcome-feature-text">
              <strong>Resume live sessions</strong>
              <span>Filter or use ↑↓ and ↵ to jump straight into chat.</span>
            </div>
          </div>
          <div className="welcome-feature">
            <span className="welcome-feature-icon">
              <Icon name="shield" size={16} />
            </span>
            <div className="welcome-feature-text">
              <strong>Local-first by design</strong>
              <span>Nothing is uploaded; only your chosen agent sees your context.</span>
            </div>
          </div>
        </div>

        <div className="welcome-actions">
          <button ref={startRef} type="button" className="btn primary" onClick={onDismiss}>
            Start
          </button>
        </div>
        <p className="welcome-hint">
          Press <kbd>{glyph}</kbd> again to start — no click needed.
        </p>
      </div>
    </div>
  );
}
