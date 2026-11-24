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

const TABS: { id: SettingsTab; label: string; icon: string; sq: string }[] = [
  { id: "general", label: "General", icon: "settings", sq: "sq-gen" },
  { id: "agents", label: "Agents", icon: "agentLocal", sq: "sq-agt" },
  { id: "privacy", label: "Privacy", icon: "shield", sq: "sq-prv" },
  { id: "about", label: "About", icon: "info", sq: "sq-abt" },
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
  const [detectedSessions, setDetectedSessions] = useState<Map<string, LiveSession[]>>(
    () => new Map()
  );
  const [detectingId, setDetectingId] = useState<string | null>(null);
  const [confirmingRemove, setConfirmingRemove] = useState<string | null>(null);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [error, setError] = useState("");
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

  const applyTab = useCallback((raw: unknown) => {
    if (TABS.some((t) => t.id === raw)) setTab(raw as SettingsTab);
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("settings:tab", (e) => applyTab(e.payload)).then((fn) => {
      unlisten = fn;
    });
    void takeSettingsTab().then(applyTab);
    return () => {
      unlisten?.();
    };
  }, [applyTab]);

  useEffect(() => {
    if (!confirmingRemove) return;
    const t = window.setTimeout(() => setConfirmingRemove(null), 3000);
    return () => window.clearTimeout(t);
  }, [confirmingRemove]);

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
        <div className="settings-traffic-strip" data-tauri-drag-region>
          <span className="settings-brand-fallback" data-tauri-drag-region>
            <HandoverMark size={15} />
            <span>Handover</span>
          </span>
        </div>
        <div className="settings-nav-list">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              className={`settings-nav${tab === t.id ? " active" : ""}`}
              onClick={() => setTab(t.id)}
              aria-current={tab === t.id ? "page" : undefined}
            >
              <span className={`sq-icon ${t.sq}`} aria-hidden="true">
                <Icon name={t.icon} size={12} />
              </span>
              <span>{t.label}</span>
            </button>
          ))}
        </div>
      </nav>

      <main className="settings-main">
        {tab === "general" && info && (
          <>
            <div className="settings-head" data-tauri-drag-region>
              <h1 className="settings-title" data-tauri-drag-region>
                General
              </h1>
              <span className="settings-sub" data-tauri-drag-region>
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
              <h1 className="settings-title" data-tauri-drag-region>
                Agents
              </h1>
              <span className="settings-sub" data-tauri-drag-region>
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
              <h1 className="settings-title" data-tauri-drag-region>
                Privacy
              </h1>
              <span className="settings-sub" data-tauri-drag-region>
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
          <>
            <div className="settings-head" data-tauri-drag-region>
              <h1 className="settings-title" data-tauri-drag-region>
                About
              </h1>
            </div>
            <AboutSection info={info} agents={agents} onError={setError} />
          </>
        )}
      </main>
    </div>
  );
}
