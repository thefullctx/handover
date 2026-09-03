import { useEffect, useRef } from "react";
import { Icon } from "../Icons";
import { formatShortcut } from "../lib/format";

interface Props {
  /** The configured accelerator, e.g. `CmdOrCtrl+Shift+A`. */
  shortcut: string;
  onClose: () => void;
}

/** A single row: one or more keycaps + what they do. */
function Row({ keys, action }: { keys: string[]; action: string }) {
  return (
    <div className="sheet-row">
      <span className="sheet-keys">
        {keys.map((k, i) => (
          <kbd key={i}>{k}</kbd>
        ))}
      </span>
      <span className="sheet-action">{action}</span>
    </div>
  );
}

/**
 * The palette's keyboard cheat sheet (Google-Docs-⌘/-style, but quieter):
 * a compact overlay listing the shortcuts that matter, opened from the
 * footer button or by pressing `?` when not typing.
 */
export default function ShortcutSheet({ shortcut, onClose }: Props) {
  const glyph = formatShortcut(shortcut);
  const closeRef = useRef<HTMLButtonElement>(null);

  // Focus the close button on open (a11y) so Tab can't roam behind the modal;
  // the App-level Escape handler closes it and restores focus to the palette.
  useEffect(() => {
    closeRef.current?.focus();
  }, []);

  const onOverlayKey = (e: React.KeyboardEvent) => {
    if (e.key === "Tab") {
      e.preventDefault();
      closeRef.current?.focus();
    }
  };

  return (
    <div
      className="sheet-overlay"
      role="dialog"
      aria-modal="true"
      aria-label="Keyboard shortcuts"
      data-testid="shortcut-sheet"
      onKeyDown={onOverlayKey}
    >
      <div className="sheet-card">
        <div className="sheet-head">
          <span className="sheet-title">
            <Icon name="keyboard" size={14} />
            Keyboard shortcuts
          </span>
          <button
            ref={closeRef}
            type="button"
            className="icon-btn"
            aria-label="Close keyboard shortcuts"
            onClick={onClose}
          >
            <Icon name="close" size={13} />
          </button>
        </div>

        <div className="sheet-rows">
          <Row keys={[glyph]} action="Open the palette from anywhere" />
          <Row keys={["↑", "↓"]} action="Choose an agent to chat with" />
          <Row keys={["↵"]} action="Chat with the selected agent" />
          <Row keys={["⌘", "↵"]} action="Send a chat message" />
          <Row keys={["⌘", ","]} action="Open Settings" />
          <Row keys={["esc"]} action="Dismiss — a running handoff keeps going" />
        </div>

        <p className="sheet-note">
          Handover runs locally — your context goes only to the agent you choose.
        </p>
      </div>
    </div>
  );
}
