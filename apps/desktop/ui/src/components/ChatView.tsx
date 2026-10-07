import { useEffect, useRef, useState } from "react";
import { copyText } from "../lib/tauri";
import { Markdown } from "../lib/markdown";
import { Icon } from "../Icons";
import { byteLength, captureSnippet, cleanReply, failureMessage, fmtBytes, fmtDuration, fmtMs, PHASE_STEPS, phaseIndex, sessionTimeLabel, truncate } from "../lib/format";
import type { AgentMeta, ApprovalResult, ChatTurn, HandoffActivity, LiveSession } from "../lib/types";
import ApprovalCard from "./ApprovalCard";

interface Props {
  /** The agent being chatted with (named in bubbles + composer). */
  agent: AgentMeta;
  /** The live session being resumed. `null` = no session to resume — each
   *  message falls back to the daemon's normal resolution (honest chip). */
  sessionId: string | null;
  /** The agent's freshest live session (drives the blocked approval card). */
  session?: LiveSession | null;
  /** The conversation so far, oldest first. */
  turns: ChatTurn[];
  /** Text of the message currently in flight (renders the in-flight user
   *  bubble). `null` = nothing sending. */
  sendingText: string | null;
  elapsed: number;
  /** Real measurements for the collapsible "Show activity" section (phase
   *  timeline, prompt size, time to first response, output volume). `null`
   *  when there is nothing to show yet. */
  activity?: HandoffActivity | null;
  /** Composer prefill (captured clipboard / dropped context). Re-applied
   *  whenever `draftSignal` changes. */
  draft?: string;
  draftSignal?: number;
  /** Approve/deny when the resumed session is parked at an approval prompt
   *  (only the explicit button press injects). */
  onApprove?: (approve: boolean) => void;
  approving?: boolean;
  approvalNote?: ApprovalResult | null;
  onSend: (text: string) => void;
}

/**
 * Builds the activity payload for a *finished* turn.
 *
 * Output volume and phase are derivable from the outcome itself, so they are
 * derived here rather than stored. Prompt size comes from the daemon's own
 * measurement when the turn streamed; turns reopened from history never
 * streamed, so it falls back to measuring the prompt we do have. Time to first
 * response is the one value that cannot be recovered after the fact and is
 * carried on the turn itself.
 *
 * Returns `null` when there is nothing worth showing, so a turn with no
 * receipt and no measurements renders no panel rather than an empty one.
 */
function turnActivity(turn: ChatTurn): HandoffActivity | null {
  const output = [turn.outcome.receipt?.stdout ?? "", turn.outcome.receipt?.stderr ?? ""]
    .filter(Boolean)
    .join("\n");
  const promptBytes =
    turn.promptBytes ?? (turn.prompt ? byteLength(turn.prompt) : null);
  if (!output.trim() && promptBytes === null && turn.firstResponseMs == null) {
    return null;
  }
  return {
    phase: "done",
    promptBytes,
    firstResponseMs: turn.firstResponseMs ?? null,
    outputBytes: byteLength(output),
    output,
  };
}

/** The collapsible "Show activity" panel — phase timeline, measured stats and
 *  the raw agent output. Rendered per turn so each exchange reports its own
 *  numbers, and live on the in-flight turn while it streams. */
function ActivitySection({ activity, testId }: { activity: HandoffActivity; testId: string }) {
  return (
    <details className="chat-activity" data-testid={testId}>
      <summary>
        <Icon name="activity" size={12} />
        Activity
      </summary>
      <div className="activity-body">
        {/* Phase timeline — the current phase is derived from the output
            stream in App.tsx, never animated on a timer. */}
        <ol className="activity-phases" data-testid={`${testId}-phases`}>
          {PHASE_STEPS.map((step, idx) => {
            const active = phaseIndex(activity.phase) === idx;
            const done = activity.phase === "done" || phaseIndex(activity.phase) > idx;
            return (
              <li
                key={step.id}
                className={`activity-phase${active ? " active" : ""}${done ? " done" : ""}`}
                data-active={active ? "true" : "false"}
              >
                {step.label}
              </li>
            );
          })}
        </ol>
        <div className="activity-stats">
          <span className="activity-stat">
            <span className="activity-stat-label">Prompt</span>
            <span data-testid={`${testId}-prompt-size`}>
              {activity.promptBytes === null ? "—" : fmtBytes(activity.promptBytes)}
            </span>
          </span>
          <span className="activity-stat">
            <span className="activity-stat-label">First response</span>
            <span data-testid={`${testId}-first-response`}>
              {activity.firstResponseMs === null ? "—" : fmtMs(activity.firstResponseMs)}
            </span>
          </span>
          <span className="activity-stat">
            <span className="activity-stat-label">Output</span>
            <span data-testid={`${testId}-output-volume`}>{fmtBytes(activity.outputBytes)}</span>
          </span>
        </div>
        {activity.output.trim() && (
          <pre className="activity-output" data-testid={`${testId}-output`}>
            {truncate(activity.output, 4000)}
          </pre>
        )}
      </div>
    </details>
  );
}

/**
 * The palette's live chat: a threaded conversation that keeps resuming the
 * SAME session (`session_id` is passed explicitly on every send, never left
 * to freshest-resolution). Your message is plain right-aligned text; the
 * reply reads as formatted Markdown with a quiet line under it (agent · time ·
 * duration, Copy, Retry, Activity) that shows on hover and always on the
 * newest reply. The exact rendered prompt stays one step away in Recent
 * handoffs (Copy prompt). While a message is in flight the thread shows only
 * the thinking state; completion appends the turn in place.
 */
export default function ChatView({
  agent,
  session,
  turns,
  sendingText,
  elapsed,
  activity = null,
  draft,
  draftSignal = 0,
  onApprove,
  approving = false,
  approvalNote,
  onSend,
}: Props) {
  const [text, setText] = useState("");
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [copiedTurn, setCopiedTurn] = useState<number | null>(null);
  const sending = sendingText !== null;
  const blocked = !!session?.blocked;

  // Enter the chat with the composer focused; stay pinned to the newest turn.
  useEffect(() => {
    editorRef.current?.focus();
  }, []);
  // Prefill the composer from captured/dropped context (also on new drops).
  useEffect(() => {
    if (draft !== undefined) setText(draft);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftSignal]);
  // The composer grows with its text (up to the CSS max-height), then scrolls.
  useEffect(() => {
    const el = editorRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  // Glide to the newest turn (jump when the user prefers reduced motion).
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    if (typeof el.scrollTo === "function") {
      el.scrollTo({ top: el.scrollHeight, behavior: reduce ? "auto" : "smooth" });
    } else {
      el.scrollTop = el.scrollHeight;
    }
  }, [turns.length, sending]);

  const copyReply = (i: number, reply: string) => {
    if (!reply) return;
    void copyText(reply);
    setCopiedTurn(i);
    setTimeout(() => setCopiedTurn((c) => (c === i ? null : c)), 1400);
  };

  const send = () => {
    const t = text.trim();
    if (!t || sending) return;
    setText("");
    onSend(t);
  };

  return (
    <div className="chat" data-testid="chat">
      <div className="chat-thread" ref={scrollRef} data-testid="chat-thread">
        {blocked && (
          <div className="chat-approval" data-testid="chat-approval">
            <ApprovalCard
              agentName={agent.name}
              question={session?.blocked_detail}
              approving={approving}
              onApprove={onApprove}
              note={approvalNote}
            />
          </div>
        )}

        {turns.length === 0 && !sending && (
          <p className="chat-empty">
            {draft
              ? "Your captured context is in the box below — edit or send it."
              : `Say hello to ${agent.name} — or send a question.`}
          </p>
        )}
        {turns.map((turn, i) => {
          const mine = turn.outcome.capture?.content.text ?? captureSnippet(turn.outcome.capture ?? null);
          const reply = cleanReply(turn.outcome.receipt?.stdout ?? "");
          const ok = turn.outcome.ok;
          // Each turn reports its OWN measurements — a single global panel
          // made turn 1 display turn 2's numbers.
          const activity = turnActivity(turn);
          const latest = i === turns.length - 1 && !sending;
          return (
            <div className={`chat-turn${latest ? " latest" : ""}`} key={i}>
              <div className="chat-row user">
                <div className="chat-message">
                  <span className="chat-text">{truncate(mine, 8000) || "—"}</span>
                </div>
              </div>
              <div className={`chat-reply${ok ? "" : " fail"}`}>
                {ok ? (
                  reply ? (
                    <div className="chat-md" data-testid="chat-reply">
                      <Markdown text={truncate(reply, 8000)} />
                    </div>
                  ) : (
                    <p className="chat-text muted">
                      The agent finished without returning text output.
                    </p>
                  )
                ) : (
                  <>
                    <p className="chat-text error">{failureMessage(turn.outcome)}</p>
                    {(turn.outcome.receipt?.stderr?.trim() ||
                      turn.outcome.receipt?.stdout?.trim()) && (
                      <details className="chat-details">
                        <summary>Agent output</summary>
                        <pre className="err">
                          {truncate(
                            turn.outcome.receipt?.stderr?.trim() ||
                              turn.outcome.receipt?.stdout?.trim() ||
                              "",
                            4000
                          )}
                        </pre>
                      </details>
                    )}
                  </>
                )}
                <div className="reply-meta" data-testid={`reply-meta-${i}`}>
                  <span className="reply-info">
                    {agent.name}
                    <span className="dot-sep">·</span>
                    {turn.outcome.created_at
                      ? sessionTimeLabel(turn.outcome.created_at)
                      : "just now"}
                    {turn.outcome.receipt?.duration_ms != null && (
                      <>
                        <span className="dot-sep">·</span>
                        <span title="Time to reply">
                          {fmtDuration(turn.outcome.receipt.duration_ms)}
                        </span>
                      </>
                    )}
                  </span>
                  {ok && reply && (
                    <button
                      type="button"
                      className="reply-btn"
                      onClick={() => copyReply(i, reply)}
                    >
                      <Icon name={copiedTurn === i ? "check" : "copy"} size={12} />
                      {copiedTurn === i ? "Copied" : "Copy"}
                    </button>
                  )}
                  <button
                    type="button"
                    className="reply-btn"
                    disabled={sending || !mine}
                    onClick={() => onSend(mine)}
                    title="Send the same message again"
                  >
                    <Icon name="refresh" size={12} />
                    Retry
                  </button>
                  {activity && (
                    <ActivitySection activity={activity} testId={`chat-activity-${i}`} />
                  )}
                </div>
              </div>
            </div>
          );
        })}

        {sending && (
          <div className="chat-turn" data-testid="chat-inflight">
            <div className="chat-row user">
              <div className="chat-message">
                <span className="chat-text">{truncate(sendingText ?? "", 8000)}</span>
              </div>
            </div>
            <div className="chat-reply working">
              {/* Only the thinking state while in flight: streamed text
                  (progress, thoughts, partial reply) is never shown here.
                  The finished reply replaces it; the raw stream stays
                  available in the activity panel. */}
              <span className="chat-thinking" data-testid="chat-thinking">
                <span className="pixel-loader" aria-hidden="true" />
                Thinking…
              </span>
              <div className="reply-meta">
                <span className="reply-info">
                  <span className="status-dot ok" />
                  {agent.name} is working{elapsed > 0 ? ` · ${elapsed}s` : ""}
                </span>
                {/* The in-flight turn owns the live panel; once it completes
                    the measurements are snapshotted onto the turn itself. */}
                {activity && <ActivitySection activity={activity} testId="chat-activity" />}
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="followup-composer">
        <textarea
          ref={editorRef}
          className="followup-editor"
          aria-label="Chat message"
          placeholder={`Message ${agent.name}…`}
          spellCheck={false}
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter sends; Shift+Enter is the newline. Cmd/Ctrl+Enter also
            // sends (matches the `?` cheat sheet) — chat feels like a messenger.
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <button
          type="button"
          className="icon-btn solid"
          aria-label="Send message"
          disabled={!text.trim() || sending}
          onClick={send}
        >
          <Icon name="chevronRight" size={16} />
        </button>
      </div>
    </div>
  );
}
