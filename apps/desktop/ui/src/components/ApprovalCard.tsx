import { Icon } from "../Icons";
import type { ApprovalResult } from "../lib/types";

interface Props {
  /** The agent asking for a decision (shown in the title line). */
  agentName: string;
  /** What the agent is actually asking — the matched permission-marker line
   *  surfaced by the daemon. Falls back to a generic line when unavailable. */
  question?: string | null;
  /** True while the decision is in flight (buttons disabled). */
  approving?: boolean;
  /** Only the explicit button press injects. */
  onApprove?: (approve: boolean) => void;
  /** Result of the last decision (verified / fail-soft message), inline. */
  note?: ApprovalResult | null;
}

/**
 * The human-in-the-loop approval card (Beautiful UI #4 pattern): instead of a
 * bare "blocked · approve?" chip, the agent's pending question is shown as a
 * card with explicit Approve / Deny actions. Only the button press injects —
 * the daemon re-checks the session is still blocked and verifies the
 * transcript resumes before ever claiming success.
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
        <Icon name="shield" size={16} />
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
            className="btn"
            onClick={() => onApprove?.(true)}
            disabled={approving || !onApprove}
            aria-label="Approve"
          >
            <Icon name="check" size={13} />
            Approve
          </button>
          <button
            type="button"
            className="btn ghost"
            onClick={() => onApprove?.(false)}
            disabled={approving || !onApprove}
            aria-label="Deny"
          >
            Deny
          </button>
        </span>
      </div>
    </div>
  );
}
