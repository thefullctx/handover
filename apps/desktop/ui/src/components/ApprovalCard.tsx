import { Icon } from "../Icons";
import type { ApprovalResult } from "../lib/types";

interface Props {
  /** The agent asking for a decision. */
  agentName: string;
  /** The matched permission-marker line surfaced by the daemon. */
  question?: string | null;
  /** True while the decision is in flight (buttons disabled). */
  approving?: boolean;
  /** Only the explicit button press injects. */
  onApprove?: (approve: boolean) => void;
  /** Result of the last decision, inline. */
  note?: ApprovalResult | null;
}

/**
 * Native macOS-style approval card (secondary Deny left, primary Approve right).
 */
export default function ApprovalCard({
  agentName,
  question,
  approving = false,
  onApprove,
  note,
}: Props) {
  return (
    <div className="approval-card" data-testid="approval-card" role="group" aria-label="Approval needed">
      <span className="approval-card-icon">
        <Icon name="shield" size={15} />
      </span>
      <div className="approval-card-body">
        <span className="approval-card-title">
          {agentName} needs approval
          {approving && <span className="approval-card-spinner" aria-hidden="true" />}
        </span>
        <span className="approval-card-question">
          {question?.trim() || "This session is waiting for a decision."}
        </span>
        {note && (
          <span
            className={`approval-note ${note.verified ? "ok" : "warn"}`}
            data-testid="approval-note"
          >
            {note.message}
          </span>
        )}
        <span className="approval-card-actions" role="group" aria-label="Approval decision">
          <button
            type="button"
            className="btn ghost"
            onClick={() => onApprove?.(false)}
            disabled={approving || !onApprove}
            aria-label="Deny"
          >
            Deny
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => onApprove?.(true)}
            disabled={approving || !onApprove}
            aria-label="Approve"
          >
            <Icon name="check" size={13} />
            Approve
          </button>
        </span>
      </div>
    </div>
  );
}
