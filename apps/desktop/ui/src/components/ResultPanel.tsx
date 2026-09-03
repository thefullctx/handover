import { Icon } from "../Icons";
import { cleanReply, failureMessage, fmtDuration, outcomeTitle, truncate } from "../lib/format";
import type { SendOutcome } from "../lib/types";
import { DetailChevron } from "./ui";

interface Props {
  outcome: SendOutcome;
  /** True when this panel was opened from history (Back instead of Close). */
  fromHistory: boolean;
  onBack: () => void;
  onCopyReply: () => void;
  onCopyPrompt: () => void;
  onChat: () => void;
  onRepeatWithAnotherAgent: () => void;
  onClose: () => void;
}

/**
 * A completed handoff reads as a resolved loop: a human outcome title first
 * ("Fix analysis completed"), then the answer as readable text. Raw stdout,
 * stderr and the prompt stay in expandable details. Errors are plain-language
 * with Retry — never raw stack traces.
 */
export default function ResultPanel({
  outcome,
  fromHistory,
  onBack,
  onCopyReply,
  onCopyPrompt,
  onChat,
  onRepeatWithAnotherAgent,
  onClose,
}: Props) {
  const stdout = cleanReply(outcome.receipt?.stdout ?? "");
  const stderr = outcome.receipt?.stderr?.trim() ?? "";
  const dur = outcome.receipt ? fmtDuration(outcome.receipt.duration_ms) : "—";
  const ok = outcome.ok;

  return (
    <div className="result-panel" data-testid="result-panel">
      <div className="result-head">
        <div className={`result-badge ${ok ? "ok" : "fail"}`}>
          <Icon name={ok ? "check" : "close"} size={17} />
        </div>
        <div className="result-meta">
          <h2 className="result-title">{outcomeTitle(outcome)}</h2>
          <div className="result-sub">
            <span>{outcome.agent_name}</span>
            <span className="dot-sep">·</span>
            <span>{dur}</span>
            <span className="dot-sep">·</span>
            <span>{outcome.action_id}</span>
          </div>
        </div>
      </div>

      {ok ? (
        <div className="answer-box">
          {stdout ? (
            <pre className="answer-text">{truncate(stdout, 8000)}</pre>
          ) : (
            <p className="answer-empty">The agent finished without returning text output.</p>
          )}
        </div>
      ) : (
        <div className="answer-box">
          <p className="error-text">{failureMessage(outcome)}</p>
        </div>
      )}

      {!ok && (stdout || stderr) && (
        <details className="result-details">
          <summary>
            <DetailChevron /> Agent output
          </summary>
          <pre className="err">{truncate(stderr || stdout, 6000)}</pre>
        </details>
      )}

      {ok && (
        <>
          {stdout && stdout.length > 8000 && (
            <details className="result-details">
              <summary>
                <DetailChevron /> Full output
              </summary>
              <pre>{truncate(stdout, 20000)}</pre>
            </details>
          )}
          {stderr && (
            <details className="result-details">
              <summary>
                <DetailChevron /> Errors (stderr)
              </summary>
              <pre className="err">{truncate(stderr, 6000)}</pre>
            </details>
          )}
        </>
      )}

      <details className="result-details">
        <summary>
          <DetailChevron /> Prompt that was sent
        </summary>
        <pre className="prompt">{truncate(outcome.prompt, 20000)}</pre>
      </details>

      <div className="result-actions">
        <button
          type="button"
          className="btn primary"
          onClick={onCopyReply}
          disabled={!stdout}
          aria-label="Copy answer"
        >
          <Icon name="copy" size={13} />
          Copy answer
        </button>
        <button type="button" className="btn" onClick={onCopyPrompt}>
          Copy prompt
        </button>
        <button type="button" className="btn" onClick={onChat}>
          <Icon name="forward" size={13} />
          Keep chatting
        </button>
        <button type="button" className="btn" onClick={onRepeatWithAnotherAgent}>
          <Icon name="swap" size={13} />
          Another agent
        </button>
        {fromHistory ? (
          <button type="button" className="btn" onClick={onBack}>
            Back
          </button>
        ) : (
          <button type="button" className="btn" onClick={onClose}>
            Close
          </button>
        )}
      </div>
    </div>
  );
}
