import { useEffect, useRef, useState } from "react";
import { Icon } from "../Icons";
import { captureSnippet, cleanReply, failureMessage, fmtBytes, fmtDuration, fmtMs, filterLiveStream, PHASE_STEPS, phaseIndex, sessionTimeLabel, truncate } from "../lib/format";
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
  /** Streamed agent output while a message is in flight. */
  liveText: string;
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
 * The palette's live chat: a threaded conversation that keeps resuming the
 * SAME session (`session_id` is passed explicitly on every send, never left
 * to freshest-resolution), with the exact rendered prompt shown expandable
 * under each user bubble — what the agent received is never hidden. While a
 * message is in flight the thread shows a streaming bubble; completion appends
 * the turn in place, so a chat is many replies, not one.
 */
export default function ChatView({
  agent,
  session,
  turns,
  sendingText,
  liveText,
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
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [turns.length, liveText, sending]);

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
          return (
            <div className="chat-turn" key={i}>
              <div className="chat-row user">
                <div className="chat-bubble user">
                  <span className="chat-meta">You</span>
                  <span className="chat-text">{truncate(mine, 8000) || "—"}</span>
                  {/* Transparency: the exact prompt the agent received —
                      not what you typed, which may have been wrapped by an
                      action template. Nothing sent is ever hidden. */}
                  {turn.outcome.prompt && (
                    <details className="chat-details">
                      <summary>Prompt that was sent</summary>
                      <pre className="prompt">{truncate(turn.outcome.prompt, 8000)}</pre>
                    </details>
                  )}
                </div>
              </div>
              <div className="chat-row agent">
                <div className={`chat-bubble agent${ok ? "" : " fail"}`}>
                  <span className="chat-meta">
                    {agent.name}
                    <span className="dot-sep">·</span>
                    {turn.outcome.created_at
                      ? sessionTimeLabel(turn.outcome.created_at)
                      : "just now"}
                    {turn.outcome.receipt?.duration_ms != null && (
                      <span className="chat-dur" title="Time to reply">
                        {fmtDuration(turn.outcome.receipt.duration_ms)}
                      </span>
                    )}
                  </span>
                  {ok ? (
                    reply ? (
                      <span className="chat-text">{truncate(reply, 8000)}</span>
                    ) : (
                      <span className="chat-text muted">
                        The agent finished without returning text output.
                      </span>
                    )
                  ) : (
                    <>
                      <span className="chat-text error">
                        {failureMessage(turn.outcome)}
                      </span>
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
                </div>
              </div>
            </div>
          );
        })}

        {sending && (
          <div className="chat-turn" data-testid="chat-inflight">
            <div className="chat-row user">
              <div className="chat-bubble user">
                <span className="chat-meta">You</span>
                <span className="chat-text">{truncate(sendingText ?? "", 8000)}</span>
              </div>
            </div>
            <div className="chat-row agent">
              <div className="chat-bubble agent working">
                <span className="chat-meta">
                  <span className="status-dot ok" />
                  {agent.name} is working{elapsed > 0 ? ` · ${elapsed}s` : ""}
                </span>
                {/* Banner noise (session-resume lines) is filtered out — the
                    smooth thinking state stays up until real reply text. */}
                {filterLiveStream(liveText).trim() ? (
                  <span className="chat-text live">{truncate(filterLiveStream(liveText), 8000)}</span>
                ) : (
                  <span className="chat-thinking">
                    <span className="pixel-loader" aria-hidden="true" />
                    Thinking…
                  </span>
                )}
              </div>
            </div>
          </div>
        )}
      </div>

      {activity && (
        <details className="chat-activity" data-testid="chat-activity">
          <summary>
            <Icon name="activity" size={12} />
            Show activity
          </summary>
          <div className="activity-body">
            {/* Phase timeline — the current phase is derived from the output
                stream in App.tsx, never animated on a timer. */}
            <ol className="activity-phases" data-testid="activity-phases">
              {PHASE_STEPS.map((step) => {
                const active = phaseIndex(activity.phase) === PHASE_STEPS.indexOf(step);
                const done =
                  activity.phase === "done" || phaseIndex(activity.phase) > PHASE_STEPS.indexOf(step);
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
                <span data-testid="stat-prompt-size">
                  {activity.promptBytes === null ? "—" : fmtBytes(activity.promptBytes)}
                </span>
              </span>
              <span className="activity-stat">
                <span className="activity-stat-label">First response</span>
                <span data-testid="stat-first-response">
                  {activity.firstResponseMs === null ? "—" : fmtMs(activity.firstResponseMs)}
                </span>
              </span>
              <span className="activity-stat">
                <span className="activity-stat-label">Output</span>
                <span data-testid="stat-output-volume">{fmtBytes(activity.outputBytes)}</span>
              </span>
            </div>
            {activity.output.trim() && (
              <pre className="activity-output" data-testid="activity-output">
                {truncate(activity.output, 4000)}
              </pre>
            )}
          </div>
        </details>
      )}

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
