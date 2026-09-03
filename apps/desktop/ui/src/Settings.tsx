import { useCallback, useEffect, useState } from "react";
import { HandoverMark, Icon } from "./Icons";
import { listen } from "./lib/tauri";
import {
  addAgent,
  configureAgent,
  getAppInfo,
  getAvailableAgents,
  getConfiguredAgents,
  getLiveSessions,
  makeDefaultAgent,
  removeAgent,
  setAgentEnabled,
  takeSettingsTab,
} from "./lib/tauri";
import type {
  AgentConfig,
  AppInfo,
  GalleryAgent,
  LiveSession,
  SettingsTab,
} from "./lib/types";
import AgentList, { EMPTY_AGENT_FORM, type AgentForm } from "./settings/AgentList";
import AboutSection from "./settings/AboutSection";
import GeneralSection from "./settings/GeneralSection";
import PrivacySection from "./settings/PrivacySection";

const TABS: { id: SettingsTab; label: string; icon: string }[] = [
  { id: "general", label: "General", icon: "settings" },
  { id: "agents", label: "Agents", icon: "agentLocal" },
  { id: "privacy", label: "Privacy", icon: "shield" },
  { id: "about", label: "About", icon: "info" },
];

export default function SettingsApp() {
  const [tab, setTab] = useState<SettingsTab>("general");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [agents, setAgents] = useState<AgentConfig[]>([]);
  const [gallery, setGallery] = useState<GalleryAgent[]>([]);
  const [configuringId, setConfiguringId] = useState<string | null>(null);
  const [pathInput, setPathInput] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [form, setForm] = useState<AgentForm>(EMPTY_AGENT_FORM);
  const [saving, setSaving] = useState(false);
  /** Live sessions per agent, from the last "Detect session" click. */
  const [detectedSessions, setDetectedSessions] = useState<Map<string, LiveSession[]>>(
    () => new Map()
  );
  const [detectingId, setDetectingId] = useState<string | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState<string | null>(null);
  /** Agent id whose visibility toggle is in flight (optimistic UI). */
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [error, setError] = useState("");
  /** Excluded paths mirrored from the last save (PrivacySection owns the draft). */
  const [excludedPaths, setExcludedPaths] = useState<string[]>([]);

  const reload = useCallback(async () => {
    try {
      const [i, a, g] = await Promise.all([
        getAppInfo(),
        getConfiguredAgents(),
        getAvailableAgents(),
      ]);
      setInfo(i);
      setAgents(a);
      setGallery(g);
      setExcludedPaths(i.excluded_paths ?? []);
      setError("");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** Guard against unknown/raced tab payloads (renders the General tab). */
  const applyTab = useCallback((raw: unknown) => {
    if (TABS.some((t) => t.id === raw)) setTab(raw as SettingsTab);
  }, []);

  // The app menu / tray can request a specific tab ("About Handover" → about).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("settings:tab", (e) => applyTab(e.payload)).then((fn) => {
      unlisten = fn;
    });
    // Robust fallback: if the emit raced window load, pick up the pending tab.
    void takeSettingsTab().then(applyTab);
    return () => {
      unlisten?.();
    };
  }, [applyTab]);

  // A Remove button armed for confirmation reverts after 3s.
  useEffect(() => {
    if (!confirmingRemove) return;
    const t = window.setTimeout(() => setConfirmingRemove(null), 3000);
    return () => window.clearTimeout(t);
  }, [confirmingRemove]);

  // -------------------------------------------------------------------------
  // Agent actions
  // -------------------------------------------------------------------------

  const addFromGallery = useCallback(
    async (a: GalleryAgent) => {
      setError("");
      try {
        await configureAgent(a.id, null);
        await reload();
      } catch (e) {
        setError(String(e));
      }
    },
    [reload]
  );

  const openConfigure = useCallback((a: GalleryAgent) => {
    setConfiguringId(a.id);
    setPathInput("");
    setError("");
  }, []);

  const submitConfigure = useCallback(
    async (a: GalleryAgent) => {
      setError("");
      try {
        await configureAgent(a.id, pathInput.trim() || null);
        setConfiguringId(null);
        await reload();
      } catch (e) {
        setError(String(e));
      }
    },
    [pathInput, reload]
  );

  const makeDefault = useCallback(
    async (id: string) => {
      setError("");
      try {
        await makeDefaultAgent(id);
        await reload();
      } catch (e) {
        setError(String(e));
      }
    },
    [reload]
  );

  /** Visibility switch: ON = shown in the palette dropdown. Optimistic —
   *  the row flips instantly, then we confirm with the daemon. */
  const toggleEnabled = useCallback(
    async (id: string, enabled: boolean) => {
      setAgents((list) =>
        list.map((a) => (a.id === id ? { ...a, enabled } : a))
      );
      setTogglingId(id);
      setError("");
      try {
        await setAgentEnabled(id, enabled);
        await reload();
      } catch (e) {
        // Roll the optimistic flip back on failure.
        setAgents((list) =>
          list.map((a) => (a.id === id ? { ...a, enabled: !enabled } : a))
        );
        setError(String(e));
      } finally {
        setTogglingId(null);
      }
    },
    [reload]
  );

  const removeAgent_ = useCallback(
    async (id: string) => {
      if (confirmingRemove !== id) {
        setConfirmingRemove(id);
        return;
      }
      setConfirmingRemove(null);
      try {
        await removeAgent(id);
        await reload();
      } catch (e) {
        setError(String(e));
      }
    },
    [confirmingRemove, reload]
  );

  /** "Detect session": scan the agent's live sessions on demand (filenames
   *  + mtimes only — the discovery privacy rule) and show them inline. */
  const detectSession = useCallback(async (id: string) => {
    setDetectingId(id);
    setError("");
    try {
      const all = await getLiveSessions();
      setDetectedSessions((m) => {
        const next = new Map(m);
        next.set(id, all.filter((s) => s.agent_id === id));
        return next;
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setDetectingId(null);
    }
  }, []);

  const openAdd = useCallback(() => {
    setForm(EMPTY_AGENT_FORM);
    setError("");
    setShowAdvanced(true);
  }, []);

  const submitAgent = useCallback(async () => {
    const id = form.id.trim();
    const name = form.name.trim();
    const command = form.command.trim();
    if (!id || !name || !command) {
      setError("id, name and command are required.");
      return;
    }
    if (!/^[a-z0-9][a-z0-9-]*$/.test(id)) {
      setError("id may only contain lowercase letters, digits and dashes.");
      return;
    }
    const timeout = form.timeoutSecs ? Number(form.timeoutSecs) : null;
    if (timeout !== null && (!Number.isFinite(timeout) || timeout <= 0)) {
      setError("Timeout must be a positive number of seconds.");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await addAgent({
        id,
        name,
        command,
        description: form.description.trim() || null,
        workingDir: form.workingDir.trim() || null,
        timeoutSecs: timeout,
        enabled: form.enabled,
      });
      setShowAdvanced(false);
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }, [form, reload]);

  const defaultId = agents.find((a) => a.enabled)?.id;

  return (
    <div className="settings-window">
      <nav className="settings-sidebar" aria-label="Settings">
        {/* The Overlay title bar puts the traffic lights over this strip —
            it doubles as the window drag region (see settings.css). */}
        <div className="settings-brand" data-tauri-drag-region>
          <span className="logo" data-tauri-drag-region>
            <HandoverMark size={18} />
          </span>
          <span className="brand-title" data-tauri-drag-region>
            Handover
          </span>
        </div>
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`settings-nav${tab === t.id ? " active" : ""}`}
            onClick={() => setTab(t.id)}
            aria-current={tab === t.id ? "page" : undefined}
          >
            <Icon name={t.icon} size={15} />
            {t.label}
          </button>
        ))}
      </nav>

      <main className="settings-main">
        {tab === "general" && info && (
          <>
            <div className="settings-head" data-tauri-drag-region>
              <span className="settings-sub settings-sub-lead" data-tauri-drag-region>
                How Handover behaves
              </span>
            </div>
            <div className="settings-body">
              <GeneralSection
                info={info}
                onInfoChange={(patch) => setInfo((i) => (i ? { ...i, ...patch } : i))}
                onError={setError}
              />
              {error && <p className="manage-error">{error}</p>}
            </div>
          </>
        )}

        {tab === "agents" && (
          <>
            <div className="settings-head" data-tauri-drag-region>
              <span className="settings-sub settings-sub-lead" data-tauri-drag-region>
                {agents.length} configured · {agents.filter((a) => a.enabled).length} in palette
              </span>
            </div>
            <div className="settings-body">
              <AgentList
                gallery={gallery}
                agents={agents}
                defaultId={defaultId}
                error={error}
                saving={saving}
                showAdvanced={showAdvanced}
                form={form}
                detectedSessions={detectedSessions}
                detectingId={detectingId}
                togglingId={togglingId}
                confirmingRemove={confirmingRemove}
                configuringId={configuringId}
                pathInput={pathInput}
                onPathInput={setPathInput}
                onDetectSession={(id) => void detectSession(id)}
                onFormChange={(patch) => setForm((f) => ({ ...f, ...patch }))}
                onRemove={(id) => void removeAgent_(id)}
                onMakeDefault={(id) => void makeDefault(id)}
                onToggleEnabled={(id, enabled) => void toggleEnabled(id, enabled)}
                onAdd={(a) => void addFromGallery(a)}
                onConfigure={openConfigure}
                onSubmitConfigure={(a) => void submitConfigure(a)}
                onCancelConfigure={() => setConfiguringId(null)}
                onOpenAdd={openAdd}
                onCloseAdd={() => setShowAdvanced(false)}
                onSubmit={() => void submitAgent()}
              />
            </div>
          </>
        )}

        {tab === "privacy" && (
          <>
            <div className="settings-head" data-tauri-drag-region>
              <span className="settings-sub settings-sub-lead" data-tauri-drag-region>
                Local-first, by design
              </span>
            </div>
            <div className="settings-body">
              <PrivacySection
                excludedPaths={excludedPaths}
                onExcludedPathsChange={setExcludedPaths}
                onError={setError}
              />
              {error && <p className="manage-error">{error}</p>}
            </div>
          </>
        )}

        {tab === "about" && info && (
          <AboutSection info={info} agents={agents} onError={setError} />
        )}
      </main>
    </div>
  );
}
