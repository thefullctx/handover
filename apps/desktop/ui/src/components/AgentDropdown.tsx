import { useEffect, useRef } from "react";
import { AgentLogo, Icon } from "../Icons";
import { sessionTimeLabel, truncate } from "../lib/format";
import type { AgentMetaStatus, LiveSession } from "../lib/types";

/** One agent entry as handed to the dropdown (agent + its freshest session). */
export interface DdItem {
  agent: AgentMetaStatus;
  session: LiveSession | null;
}

export interface DdGroup {
  label: string;
  items: DdItem[];
}

interface Props {
  /** Grouped agents in keyboard order (Running now → Available → Not found). */
  groups: DdGroup[];
  /** The agent currently being chatted with (the trigger shows it). */
  selectedAgentId: string | null;
  open: boolean;
  /** Highlighted index across the FLAT item list (groups flattened). */
  highlight: number;
  onHighlight: (i: number) => void;
  onToggle: () => void;
  onClose: () => void;
  onChoose: (item: DdItem) => void;
  onOpenSettings: () => void;
  triggerRef?: React.RefObject<HTMLButtonElement>;
}

/** Green only when the agent is genuinely alive right now — its process is
 *  running, or its session is actively being written. Red otherwise. */
function isRunning(item: DdItem): boolean {
  return !!item.agent.status.running || item.session?.activity === "working";
}

/** Amber when the binary is installed but its model provider is unreachable
 *  — the one state where a send would fail with a connection error. */
function isProviderDown(item: DdItem): boolean {
  return !!item.agent.status.provider_down;
}

/** The quiet right-hand detail on a row: what Handover would resume into
 *  ("working", "session · 2:14 PM"), or that the agent is merely installed. */
function agentMeta(item: DdItem): string {
  if (!item.agent.status.available) return "not found";
  const session = item.session;
  if (session?.activity === "working") return "working";
  if (session) return `session · ${sessionTimeLabel(session.updated_at)}`;
  return "installed";
}

/**
 * The agent dropdown — replaces the old search + list landing with a single
 * smooth picker. The trigger names the agent being chatted with (logo, name,
 * one green/red light); opening it reveals every agent grouped Running now /
 * Available / Not found, keyboard-navigable (↑↓ Enter Esc). Rows are lean:
 * logo, name, the same light, and a mono detail on the right (session time,
 * "installed") — only a blocked or provider-down agent carries extra words
 * ("blocked · approve?" plus its pending question). The chat lives DIRECTLY
 * BENEATH the trigger — choosing an agent never navigates away.
 */
export default function AgentDropdown({
  groups,
  selectedAgentId,
  open,
  highlight,
  onHighlight,
  onToggle,
  onClose,
  onChoose,
  onOpenSettings,
  triggerRef,
}: Props) {
  const rootRef = useRef<HTMLDivElement>(null);

  // Click-outside closes the menu.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open, onClose]);

  // Flat list (visual order) so the highlight index matches keyboard order.
  const flat = groups.flatMap((g) => g.items);
  const selected =
    flat.find((it) => it.agent.meta.id === selectedAgentId) ?? null;

  let runningIndex = -1;
  return (
    <div className={`agent-dd${open ? " open" : ""}`} ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="agent-dd-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={
          selected
            ? `Agent: ${selected.agent.meta.name}${isRunning(selected) ? " (running)" : " (not running)"}`
            : "Choose an agent"
        }
        onClick={onToggle}
        data-testid="agent-dropdown-trigger"
      >
        {selected ? (
          <>
            <span className="row-icon logo">
              <AgentLogo id={selected.agent.meta.id} size={18} />
            </span>
            <span className="agent-dd-name">{selected.agent.meta.name}</span>
            <span
              className={`status-dot ${isRunning(selected) ? "ok" : "err"}`}
              aria-hidden="true"
            />
          </>
        ) : (
          <span className="agent-dd-placeholder">Choose an agent…</span>
        )}
        <Icon name="chevronDown" size={14} className="agent-dd-chev" />
      </button>

      {open && (
        <div className="agent-dd-menu" role="listbox" aria-label="Agents">
          {groups.map((group) => (
            <div key={group.label} className="agent-dd-group">
              <div className="home-group">{group.label}</div>
              {group.items.map((item) => {
                runningIndex += 1;
                const idx = runningIndex;
                const { agent, session } = item;
                const available = agent.status.available;
                const blocked = !!session?.blocked;
                const providerDown = available && isProviderDown(item);
                return (
                  <button
                    key={agent.meta.id}
                    type="button"
                    role="option"
                    aria-selected={idx === highlight}
                    aria-disabled={!available}
                    aria-label={`${agent.meta.name}${blocked ? " — blocked · approve?" : providerDown ? ` — ${agent.status.provider_down}` : ""}`}
                    className={`agent-dd-option${idx === highlight ? " highlighted" : ""}`}
                    onMouseEnter={() => onHighlight(idx)}
                    onClick={() => available && onChoose(item)}
                    data-testid={`agent-${agent.meta.id}`}
                  >
                    <span className="row-icon logo">
                      <AgentLogo id={agent.meta.id} size={18} />
                    </span>
                    <span className="agent-dd-option-main">
                      <span className="agent-name-line">
                        <span className="name">{agent.meta.name}</span>
                        <span
                          className={`status-dot ${blocked ? "warn" : providerDown ? "warn" : isRunning(item) ? "ok" : "err"}`}
                        />
                      </span>
                      {blocked && (
                        <span
                          className="desc question"
                          title={session?.blocked_detail ?? undefined}
                        >
                          <Icon name="shield" size={10} />
                          blocked · approve?
                          {session?.blocked_detail
                            ? ` — ${truncate(session.blocked_detail, 60)}`
                            : ""}
                        </span>
                      )}
                      {!blocked && providerDown && (
                        <span
                          className="desc question"
                          title={agent.status.provider_down ?? undefined}
                        >
                          <Icon name="warning" size={10} />
                          {truncate(agent.status.provider_down ?? "provider down", 70)}
                        </span>
                      )}
                    </span>
                    {!blocked && !providerDown && (
                      <span className="agent-meta">{agentMeta(item)}</span>
                    )}
                  </button>
                );
              })}
            </div>
          ))}
          <div className="agent-dd-footer">
            <button type="button" className="btn ghost" onClick={onOpenSettings}>
              <Icon name="settings" size={13} />
              Manage agents in Settings
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
