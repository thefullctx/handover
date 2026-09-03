import { Icon } from "../Icons";

interface Props {
  message: string;
  canRetry: boolean;
  onRetry: () => void;
  onCopyPrompt: () => void;
  onClose: () => void;
}

/**
 * Errors are plain-language and actionable. Retry re-sends the same
 * handoff; diagnostics never leak raw stack traces into the UI.
 */
export default function ErrorState({ message, canRetry, onRetry, onCopyPrompt, onClose }: Props) {
  return (
    <div className="status-body" data-testid="error-state">
      <div className="status-icon error">
        <Icon name="warning" size={20} />
      </div>
      <h2>Handoff failed</h2>
      <p className="error-text">{message}</p>
      <div className="status-actions">
        {canRetry && (
          <button type="button" className="btn primary" onClick={onRetry}>
            <Icon name="refresh" size={13} />
            Retry
          </button>
        )}
        <button type="button" className="btn" onClick={onCopyPrompt}>
          Copy prompt
        </button>
        <button type="button" className="btn" onClick={onClose}>
          Close
        </button>
      </div>
    </div>
  );
}
