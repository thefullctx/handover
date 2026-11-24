import { useEffect, useRef } from "react";
import { AgentLogo, Icon } from "../Icons";
import { sessionTimeLabel, truncate } from "../lib/format";
import type { AgentMetaStatus, LiveSession } from "../lib/types";

/** One agent entry as handed to the unfolded picker (agent + freshest session). */
export interface PickerItem {
  agent: AgentMetaStatus;
  session: LiveSession | null;
}

export interface PickerGroup {
  label: string;
  items: PickerItem[];
}

interface Props {
  /** Grouped agents in keyboard order (Running now -> Available -> Not found). */
  groups: PickerGroup[];
  /** Active filter text from the top bar. */
  filterQuery: string;
  /** Highlighted index across the FLAT item list (groups flattened). */
  highlight: number;
  onHighlight: (i: number) => void;
  onChoose: (item: PickerItem) => void;
  onClearFilter: () => void;
  onOpenSettings: () => void;
}

/** Green only when the agent is genuinely alive right now — its process is
 *  running, or its session is actively being written. */
export function isAgentRunning(item: PickerItem): boolean {
  return !!item.agent.status.running || item.session?.activity === "working";
}

/** Amber when the binary is installed but its model provider is unreachable. */
function isProviderDown(item: PickerItem): boolean {
  return !!item.agent.status.provider_down;
}

/** Right-hand detail on a row: "Working", "Session · 2:14 PM", "Installed", or "Not found". */
function agentMeta(item: PickerItem): string {
  if (!item.agent.status.available) return "Not found";
  const session = item.session;
  if (session?.activity === "working") return "Working";
  if (session) return `Session · ${sessionTimeLabel(session.updated_at)}`;
  return "Installed";
}

/**
 * Always-unfolded native macOS agent picker — Spotlight-style inset rows
 * immediately visible when Handover opens, paired with the top filter input.
 */
export default function AgentPicker({
  groups,
  filterQuery,
  highlight,
  onHighlight,
  onChoose,
  onClearFilter,
  onOpenSettings,
}: Props) {
  const optionRefs = useRef<(HTMLButtonElement | null)[]>([]);

  // Keep the keyboard-highlighted row visible when navigating with arrows.
  useEffect(() => {
    const el = optionRefs.current[highlight];
    if (el && typeof el.scrollIntoView === "function") {
      el.scrollIntoView({ block: "nearest" });
    }
  }, [highlight, groups]);

  const hasFilter = filterQuery.trim().length > 0;

  if (groups.length === 0 && hasFilter) {
    return (
      <div className="empty-state" data-testid="filter-empty">
        <span className="status-icon off" aria-hidden="true">
          <Icon name="search" size={18} />
        </span>
        <h2>No agents match “{filterQuery.trim()}”</h2>
        <p>Clear the filter to see all configured agents, or manage them in Settings.</p>
        <div className="status-actions">
          <button type="button" className="btn primary" onClick={onClearFilter}>
            Clear filter
          </button>
          <button type="button" className="btn" onClick={onOpenSettings}>
            <Icon name="settings" size={13} />
            Open Settings
          </button>
        </div>
      </div>
    );
  }

  let runningIndex = -1;
  return (
    <div id="agent-picker-list" className="agent-picker-list" role="listbox" aria-label="Agents">
      {groups.map((group) => (
        <div key={group.label} className="agent-picker-group">
          <div className="home-group">{group.label}</div>
          {group.items.map((item) => {
            runningIndex += 1;
            const idx = runningIndex;
            const { agent, session } = item;
            const available = agent.status.available;
            const blocked = !!session?.blocked;
            const providerDown = available && isProviderDown(item);
            const running = isAgentRunning(item);
            const selected = idx === highlight;
            const dotTone = !available
              ? "off"
              : blocked || providerDown
                ? "warn"
                : running
                  ? "ok"
                  : "idle";

            return (
              <button
                key={agent.meta.id}
                id={`agent-option-${agent.meta.id}`}
                ref={(el) => {
                  optionRefs.current[idx] = el;
                }}
                type="button"
                role="option"
                aria-selected={selected}
                aria-disabled={!available}
                aria-label={`${agent.meta.name}${
                  blocked
                    ? " — blocked · approve?"
                    : providerDown
                      ? ` — ${agent.status.provider_down}`
                      : ""
                }`}
                className={`agent-option${selected ? " highlighted" : ""}`}
                onMouseEnter={() => onHighlight(idx)}
                onClick={() => available && onChoose(item)}
                data-testid={`agent-${agent.meta.id}`}
              >
                <span className="row-icon logo">
                  <AgentLogo id={agent.meta.id} size={18} />
                </span>
                <span className="agent-option-main">
                  <span className="agent-name-line">
                    <span className="name">{agent.meta.name}</span>
                    <span className={`status-dot ${dotTone}`} aria-hidden="true" />
                  </span>
                  {blocked && (
                    <span
                      className="desc question"
                      title={session?.blocked_detail ?? undefined}
                    >
                      <Icon name="shield" size={10} />
                      blocked · approve?
                      {session?.blocked_detail
                        ? ` — ${truncate(session.blocked_detail, 56)}`
                        : ""}
                    </span>
                  )}
                  {!blocked && providerDown && (
                    <span
                      className="desc question"
                      title={agent.status.provider_down ?? undefined}
                    >
                      <Icon name="warning" size={10} />
                      {truncate(agent.status.provider_down ?? "provider down", 68)}
                    </span>
                  )}
                </span>
                {!blocked && !providerDown && (
                  <span className="agent-meta">
                    <span>{agentMeta(item)}</span>
                    {selected && available && (
                      <span className="return-badge" aria-hidden="true">
                        ↵
                      </span>
                    )}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}
