import { useState } from "react";
import { AgentLogo, Icon } from "../Icons";
import { sessionChipLabel } from "../lib/format";
import { copyText } from "../lib/tauri";
import type { AgentConfig, GalleryAgent, LiveSession } from "../lib/types";

export interface AgentForm {
  id: string;
  name: string;
  command: string;
  description: string;
  workingDir: string;
  timeoutSecs: string;
  enabled: boolean;
}

export const EMPTY_AGENT_FORM: AgentForm = {
  id: "",
  name: "",
  command: "",
  description: "",
  workingDir: "",
  timeoutSecs: "",
  enabled: true,
};

interface Props {
  /** Known catalog entries with detection state (installed/configured). */
  gallery: GalleryAgent[];
  /** All configured agents, including disabled ones and the demo agent. */
  agents: AgentConfig[];
  defaultId?: string;
  error: string;
  saving: boolean;
  showAdvanced: boolean;
  form: AgentForm;
  /** Live sessions found by the last "Detect session" click, keyed by agent id. */
  detectedSessions: ReadonlyMap<string, LiveSession[]>;
  /** Agent id whose detection is in flight (label instead of button). */
  detectingId: string | null;
  /** Agent id whose visibility toggle is in flight (optimistic UI). */
  togglingId: string | null;
  /** Agent id whose Remove is armed (second click confirms). */
  confirmingRemove: string | null;
  configuringId: string | null;
  pathInput: string;
  onPathInput: (v: string) => void;
  onDetectSession: (id: string) => void;
  onFormChange: (patch: Partial<AgentForm>) => void;
  onRemove: (id: string) => void;
  onMakeDefault: (id: string) => void;
  onToggleEnabled: (id: string, enabled: boolean) => void;
  onAdd: (a: GalleryAgent) => void;
  onConfigure: (a: GalleryAgent) => void;
  onSubmitConfigure: (a: GalleryAgent) => void;
  onCancelConfigure: () => void;
  onOpenAdd: () => void;
  onCloseAdd: () => void;
  onSubmit: () => void;
}

/** Quiet one-line status — the replacement for chip soup. */
function statusLine(
  entry: GalleryAgent,
  cfg: AgentConfig | undefined,
  isDefault: boolean
): string {
  if (!cfg) {
    if (!entry.detected_path) return "Not detected";
    if (entry.signed_in === true) return "Installed · signed in";
    if (entry.signed_in === false) return "Installed · not signed in";
    return "Installed";
  }
  const parts = [cfg.enabled ? "In palette" : "Hidden"];
  if (isDefault) parts.push("default");
  if (entry.signed_in === true) parts.push("signed in");
  else if (entry.signed_in === false) parts.push("not signed in");
  return parts.join(" · ");
}

/**
 * The unified assistant list — one adaptive card per agent, ever.
 *
 * Not added → green Add (or Configure… when not detected).
 * Added     → the in-palette toggle; everything else (make default, detect
 *             session, view command, remove) collapses into a ⋯ menu so the
 *             resting state of the tab is name + status line + one control.
 * Commands stay out of sight until "View command" expands them.
 */
export default function AgentList({
  gallery,
  agents,
  defaultId,
  error,
  saving,
  showAdvanced,
  form,
  detectedSessions,
  detectingId,
  togglingId,
  confirmingRemove,
  configuringId,
  pathInput,
  onPathInput,
  onDetectSession,
  onFormChange,
  onRemove,
  onMakeDefault,
  onToggleEnabled,
  onAdd,
  onConfigure,
  onSubmitConfigure,
  onCancelConfigure,
  onOpenAdd,
  onCloseAdd,
  onSubmit,
}: Props) {
  /** Which card's ⋯ menu is open. */
  const [menuFor, setMenuFor] = useState<string | null>(null);
  /** Which card's command details are expanded. */
  const [cmdFor, setCmdFor] = useState<string | null>(null);

  const byId = new Map(agents.map((a) => [a.id, a]));
  const catalogIds = new Set(gallery.map((g) => g.id));
  // Custom agents (configured but not in the catalog) join the same list.
  const customs = agents.filter((a) => !catalogIds.has(a.id) && !a.demo);
  const demos = agents.filter((a) => a.demo);

  const closeMenu = () => setMenuFor(null);

  const renderCard = (
    key: string,
    opts: {
      id: string;
      name: string;
      description?: string | null;
      entry?: GalleryAgent;
      cfg?: AgentConfig;
      demo?: boolean;
    }
  ) => {
    const { id, name, entry, cfg, demo } = opts;
    const isDefault = id === defaultId;
    const sessionAware = !!(cfg?.session_glob || cfg?.session_cli_list);
    const detected = detectedSessions.get(id);
    const menuOpen = menuFor === id;
    const cmdOpen = cmdFor === id;
    const armed = confirmingRemove === id;

    return (
      <div
        key={key}
        className={`agent-card${demo ? " demo" : ""}${cfg && !cfg.enabled ? " off" : ""}`}
        data-testid={entry ? `agent-card-${id}` : undefined}
      >
        <div className="agent-row">
          <span className="agent-avatar" aria-hidden="true">
            <AgentLogo id={id} size={18} />
          </span>
          <div className="agent-main">
            <span className="agent-name">
              {name}
              {demo && <span className="chip warn">demo</span>}
            </span>
            <span className="agent-status">
              {demo
                ? "Built-in echo agent — verifies Handover without a real CLI"
                : entry
                  ? statusLine(entry, cfg, isDefault)
                  : cfg
                    ? [cfg.enabled ? "In palette" : "Hidden", ...(isDefault ? ["default"] : [])].join(" · ")
                    : ""}
            </span>
            {/* Inline session-detection results (triggered from the ⋯ menu). */}
            {detectingId === id && (
              <span className="agent-status">
                <span className="chip off">detecting…</span>
              </span>
            )}
            {detectingId !== id && detected && detected.length > 0 && (
              <span className="agent-sessions">
                {detected.map((s) => (
                  <span
                    key={s.session_id}
                    className={`chip session ${s.activity === "working" ? "ok" : "off"}`}
                    title={`Session ${s.session_id}`}
                  >
                    <span
                      className={`status-dot ${s.activity === "working" ? "ok" : "off"}`}
                    />
                    {sessionChipLabel(s)}
                  </span>
                ))}
              </span>
            )}
            {detectingId !== id && detected && detected.length === 0 && (
              <span className="agent-status">no live sessions</span>
            )}
          </div>

          <div className="agent-actions">
            {!cfg && entry && (
              <>
                {entry.detected_path ? (
                  <button
                    type="button"
                    className="btn primary"
                    onClick={() => onAdd(entry)}
                  >
                    <Icon name="plus" size={13} />
                    Add
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn"
                    onClick={() => onConfigure(entry)}
                  >
                    Configure…
                  </button>
                )}
              </>
            )}
            {cfg && !demo && (
              <>
                <label
                  className={`toggle${togglingId === id ? " busy" : ""}`}
                  title={
                    cfg.enabled
                      ? "Shown in the palette — click to hide it (config is kept)"
                      : "Hidden from the palette — click to show it"
                  }
                >
                  <input
                    type="checkbox"
                    role="switch"
                    checked={cfg.enabled}
                    disabled={togglingId === id}
                    onChange={(e) => onToggleEnabled(id, e.target.checked)}
                    aria-label={`${name} in palette`}
                  />
                  <span className="toggle-track" aria-hidden="true" />
                </label>
                <button
                  type="button"
                  className="menu-btn"
                  aria-label={`More actions for ${name}`}
                  aria-expanded={menuOpen}
                  onClick={() => setMenuFor(menuOpen ? null : id)}
                >
                  <Icon name="dots" size={15} />
                </button>
              </>
            )}
          </div>
        </div>

        {/* Expanded command details — hidden until asked for. */}
        {cfg && cmdOpen && (
          <div className="cmd-details" data-testid={`cmd-details-${id}`}>
            <code>{cfg.command}</code>
            {cfg.resume_command && <code>resume: {cfg.resume_command}</code>}
            <button
              type="button"
              className="btn ghost"
              onClick={() =>
                void copyText(
                  [cfg.command, cfg.resume_command && `resume: ${cfg.resume_command}`]
                    .filter(Boolean)
                    .join("\n")
                )
              }
            >
              <Icon name="clipboard" size={12} />
              Copy
            </button>
          </div>
        )}

        {/* Path input for adding a not-detected catalog agent. */}
        {entry && configuringId === id && (
          <form
            className="configure-form"
            onSubmit={(e) => {
              e.preventDefault();
              onSubmitConfigure(entry);
            }}
            data-testid="configure-form"
          >
            <p className="configure-hint">
              Enter the path to the <strong>{name}</strong> binary (or a bare
              name on PATH). Handover writes the recommended command for you.
            </p>
            <div className="configure-row">
              <input
                autoFocus
                value={pathInput}
                onChange={(e) => onPathInput(e.target.value)}
                placeholder={`e.g. ~/.local/bin/${entry.binary_name}`}
                spellCheck={false}
                aria-label="Binary path"
              />
              <button type="submit" className="btn primary">
                Add
              </button>
              <button
                type="button"
                className="btn ghost"
                onClick={onCancelConfigure}
              >
                Cancel
              </button>
            </div>
          </form>
        )}

        {/* The ⋯ overflow menu. */}
        {menuOpen && (
          <>
            <div className="menu-backdrop" onClick={closeMenu} />
            <div className="menu-pop" role="menu" aria-label={`${name} actions`}>
              {cfg && !demo && (
                <>
                  {isDefault ? (
                    <span className="menu-note">Default agent</span>
                  ) : (
                    cfg.enabled && (
                      <button
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          onMakeDefault(id);
                          closeMenu();
                        }}
                      >
                        <Icon name="check" size={13} />
                        Make default
                      </button>
                    )
                  )}
                  {sessionAware && (
                    <button
                      type="button"
                      role="menuitem"
                      disabled={detectingId === id}
                      onClick={() => {
                        onDetectSession(id);
                        closeMenu();
                      }}
                    >
                      <Icon name="layers" size={13} />
                      Detect session
                    </button>
                  )}
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setCmdFor(cmdOpen ? null : id);
                      closeMenu();
                    }}
                  >
                    <Icon name="terminal" size={13} />
                    {cmdOpen ? "Hide command" : "View command"}
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    className={armed ? "danger armed" : "danger"}
                    onClick={() => {
                      if (!armed) {
                        // First click arms; the parent's 3s timer reverts.
                        onRemove(id);
                        return;
                      }
                      onRemove(id);
                      closeMenu();
                    }}
                  >
                    <Icon name="close" size={13} />
                    {armed ? "Confirm remove" : "Remove"}
                  </button>
                </>
              )}
            </div>
          </>
        )}
      </div>
    );
  };

  return (
    <>
      {agents.length === 0 && gallery.every((g) => !g.configured) && (
        <p className="nothing" style={{ textAlign: "left", padding: "8px 4px" }}>
          Pick an assistant to start — anything already installed on this Mac is
          one tap away.
        </p>
      )}

      <div className="agent-list">
        {/* Catalog four, in catalog order; merged with their config state. */}
        {gallery.map((entry) =>
          renderCard(`gal-${entry.id}`, {
            id: entry.id,
            name: entry.name,
            entry,
            cfg: byId.get(entry.id),
          })
        )}
        {/* Custom agents, then the demo agent, always last and muted. */}
        {customs.map((cfg) =>
          renderCard(`cus-${cfg.id}`, {
            id: cfg.id,
            name: cfg.name,
            cfg,
          })
        )}
        {demos.map((cfg) =>
          renderCard(`demo-${cfg.id}`, { id: cfg.id, name: cfg.name, cfg, demo: true })
        )}
      </div>

      {error && <p className="manage-error">{error}</p>}

      <div className="settings-actions">
        {!showAdvanced ? (
          <button type="button" className="btn ghost" onClick={onOpenAdd}>
            <Icon name="plus" size={13} />
            Add custom agent…
          </button>
        ) : (
          <form
            className="agent-form"
            onSubmit={(e) => {
              e.preventDefault();
              onSubmit();
            }}
            data-testid="agent-form"
          >
            <div className="settings-section">Add a custom agent</div>
            <div className="form-body">
              <label className="field">
                <span className="field-label">Name *</span>
                <input
                  value={form.name}
                  onChange={(e) => onFormChange({ name: e.target.value })}
                  placeholder="e.g. My Agent"
                  spellCheck={false}
                  autoFocus
                />
              </label>
              <label className="field">
                <span className="field-label">
                  id * <span className="field-hint">lowercase, dashes</span>
                </span>
                <input
                  value={form.id}
                  onChange={(e) => onFormChange({ id: e.target.value })}
                  placeholder="e.g. my-agent"
                  spellCheck={false}
                />
              </label>
              <label className="field">
                <span className="field-label">Command *</span>
                <input
                  value={form.command}
                  onChange={(e) => onFormChange({ command: e.target.value })}
                  placeholder={'my-agent --prompt "{PROMPT}" or my-agent for stdin'}
                  spellCheck={false}
                />
              </label>
              <label className="field">
                <span className="field-label">Description</span>
                <input
                  value={form.description}
                  onChange={(e) => onFormChange({ description: e.target.value })}
                  placeholder="What does this agent do?"
                />
              </label>
              <label className="field">
                <span className="field-label">Working directory</span>
                <input
                  value={form.workingDir}
                  onChange={(e) => onFormChange({ workingDir: e.target.value })}
                  placeholder="e.g. ~/Projects (optional)"
                  spellCheck={false}
                />
              </label>
              <label className="field">
                <span className="field-label">Timeout (seconds)</span>
                <input
                  value={form.timeoutSecs}
                  onChange={(e) => onFormChange({ timeoutSecs: e.target.value })}
                  placeholder="120"
                  inputMode="numeric"
                />
              </label>
              <label className="field check">
                <input
                  type="checkbox"
                  checked={form.enabled}
                  onChange={(e) => onFormChange({ enabled: e.target.checked })}
                />
                <span>Enabled (available in the palette)</span>
              </label>
            </div>
            <div className="settings-actions">
              <button type="button" className="btn ghost" onClick={onCloseAdd}>
                Close
              </button>
              <button type="submit" className="btn primary" disabled={saving}>
                {saving ? "Saving…" : "Save agent"}
              </button>
            </div>
          </form>
        )}
      </div>
    </>
  );
}
