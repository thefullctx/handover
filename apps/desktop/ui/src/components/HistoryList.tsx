import { Icon } from "../Icons";
import { fmtDuration, groupHistoryByDay, kindIcon, replySnippet, truncate } from "../lib/format";
import type { Capture, SendOutcome } from "../lib/types";

interface Props {
  history: SendOutcome[];
  onOpen: (o: SendOutcome) => void;
  onRetry: (o: SendOutcome) => void;
  onCopyResult: (o: SendOutcome) => void;
  onResendWithAnother: (o: SendOutcome) => void;
  onClear: () => void;
}

/**
 * History as product memory, not a technical log. Entries group by day
 * (Today / Yesterday when persistence exists; session entries are Today),
 * each row shows action · agent · source · duration · success and a concise
 * outcome snippet, with quiet per-row actions.
 */
export default function HistoryList({
  history,
  onOpen,
  onRetry,
  onCopyResult,
  onResendWithAnother,
  onClear,
}: Props) {
  const groups = groupHistoryByDay(history);

  return (
    <div className="history-list" data-testid="history-list">
      {groups.length === 0 && (
        <div className="history-empty">
          <p>No handoffs yet this session.</p>
          <p style={{ fontSize: 11.5, marginTop: 6, color: "var(--text-tertiary)" }}>
            History is kept in memory while Handover runs — persistence with
            retention is planned.
          </p>
        </div>
      )}          {groups.map((group) => (
        <div key={group.label} className="history-group">
          <div className="history-group-label">{group.label}</div>
          {group.entries.map((o) => (
            <HistoryRow
              key={o.id}
              outcome={o}
              onOpen={onOpen}
              onRetry={onRetry}
              onCopyResult={onCopyResult}
              onResendWithAnother={onResendWithAnother}
            />
          ))}
        </div>
      ))}
      {history.length > 0 && (
        <div className="settings-actions" style={{ justifyContent: "flex-start" }}>
          <button type="button" className="btn ghost" onClick={onClear}>
            <Icon name="trash" size={12} />
            Clear history
          </button>
        </div>
      )}
    </div>
  );
}

function HistoryRow({
  outcome: o,
  onOpen,
  onRetry,
  onCopyResult,
  onResendWithAnother,
}: {
  outcome: SendOutcome;
  onOpen: (o: SendOutcome) => void;
  onRetry: (o: SendOutcome) => void;
  onCopyResult: (o: SendOutcome) => void;
  onResendWithAnother: (o: SendOutcome) => void;
}) {
  const sourceKind = sourceKindOf(o.capture);
  return (
    <div
      className="history-row"
      onClick={() => onOpen(o)}
      data-testid={`history-${o.id}`}
    >
      <button
        type="button"
        className={`history-status ${o.ok ? "ok" : "fail"}`}
        title="Open result"
        aria-label={`Open result of ${o.id}`}
        onClick={() => onOpen(o)}
        data-testid={`history-open-${o.id}`}
      >
        <Icon name={o.ok ? "check" : "close"} size={12} />
      </button>
      <span className="history-main">
        <span className="history-line">
          {o.action_id} · {o.agent_name}
        </span>
        <span className="history-snippet">{truncate(replySnippet(o), 90) || "—"}</span>
      </span>
      {sourceKind && (
        <span className="row-icon" style={{ width: 20, height: 20 }}>
          <Icon name={kindIcon(sourceKind)} size={11} />
        </span>
      )}
      <span className="history-dur">{o.receipt ? fmtDuration(o.receipt.duration_ms) : ""}</span>
      <span className="history-actions" onClick={(e) => e.stopPropagation()}>
        <button
          type="button"
          className="icon-btn"
          title="Retry (same agent)"
          aria-label={`Retry ${o.id}`}
          onClick={() => onRetry(o)}
        >
          <Icon name="refresh" size={12} />
        </button>
        <button
          type="button"
          className="icon-btn"
          title="Copy result"
          aria-label={`Copy result of ${o.id}`}
          onClick={() => onCopyResult(o)}
        >
          <Icon name="doc" size={12} />
        </button>
        <button
          type="button"
          className="icon-btn"
          title="Send to a different agent"
          aria-label={`Send ${o.id} to another agent`}
          onClick={() => onResendWithAnother(o)}
        >
          <Icon name="swap" size={12} />
        </button>
      </span>
    </div>
  );
}

function sourceKindOf(capture?: Capture | null) {
  return capture?.content.type ?? null;
}
