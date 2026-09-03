import { useEffect, useRef } from "react";
import { HandoverMark, Icon } from "../Icons";
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
 * First-run onboarding — one screen, one message, taught by doing.
 * The way to dismiss it IS the shortcut: pressing it again (or Start)
 * closes the overlay and drops you into the sheet. No wizard, no reading list.
 *
 * Before the hotkey lesson there is ONE optional step: "pick your
 * assistants". Detected (installed) agents sit at the top with a one-tap
 * Add; everything else lives in Settings. Skipping costs nothing — the
 * palette's empty state links onward, so onboarding never blocks usage.
 */
export default function WelcomeOverlay({
  shortcut,
  gallery,
  onAddAgent,
  onDismiss,
}: Props) {
  const glyph = formatShortcut(shortcut);
  const startRef = useRef<HTMLButtonElement>(null);

  // Only un-added entries are pickable here; installed ones lead.
  const candidates = [...gallery]
    .filter((a) => !a.configured)
    .sort((a, b) => Number(!!b.detected_path) - Number(!!a.detected_path));
  // The picker step only earns its space when there is something to pick.
  const showPicker = candidates.length > 0;

  // Move focus into the dialog (a11y): the overlay answers one question, and
  // Start is the answer. Tab stays on it rather than roaming behind the modal.
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
          <HandoverMark size={30} />
        </span>
        <h2>Welcome to Handover</h2>
        <p className="welcome-lead">
          See something worth handing off? Copy it, press{" "}
          <kbd className="welcome-kbd">{glyph}</kbd>, pick the agent you're
          already talking to — and keep chatting.
        </p>

        {showPicker && (
          <>
            <div className="welcome-pickers" data-testid="welcome-pickers">
              <div className="welcome-picker-title">Pick your assistants</div>
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
                  <Icon name="agentLocal" size={14} />
                  <span className="welcome-agent-name">{a.name}</span>
                  {a.signed_in === true && (
                    <span className="chip ok">signed in</span>
                  )}
                  {a.detected_path ? (
                    <span className="welcome-agent-add">
                      <Icon name="plus" size={11} />
                      Add
                    </span>
                  ) : (
                    <span className="chip off">not found</span>
                  )}
                </button>
              ))}
            </div>
            <p className="welcome-hint" style={{ marginTop: -6 }}>
              Added agents appear in the palette instantly. More options live in
              Settings → Agents.
            </p>
          </>
        )}

        <div className="welcome-steps" aria-hidden="true">
          <div className="welcome-step">
            <span className="step-num">1</span>
            <span>Copy the context</span>
          </div>
          <div className="welcome-step">
            <span className="step-num">2</span>
            <span>Pick your agent</span>
          </div>
          <div className="welcome-step">
            <span className="step-num">3</span>
            <span>Chat</span>
          </div>
        </div>

        <div className="welcome-actions">
          <button ref={startRef} type="button" className="btn primary" onClick={onDismiss}>
            Start
          </button>
        </div>
        <p className="welcome-hint">
          Tip: press <kbd>{glyph}</kbd> again to start — no click needed.
        </p>
        <p className="welcome-local">
          Runs locally · Handover does not upload your context
        </p>
      </div>
    </div>
  );
}
