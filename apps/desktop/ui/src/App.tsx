import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { AgentLogo, HandoverMark, Icon } from "./Icons";
import { usePaletteSize } from "./lib/usePaletteSize";
import AgentPicker, { isAgentRunning, type PickerGroup, type PickerItem } from "./components/AgentPicker";
import ChatView from "./components/ChatView";
import ErrorState from "./components/ErrorState";
import HistoryList from "./components/HistoryList";
import ResultPanel from "./components/ResultPanel";
import ShortcutSheet from "./components/ShortcutSheet";
import WelcomeOverlay from "./components/WelcomeOverlay";
import { captureSnippet, byteLength, failureMessage, fmtDuration, outcomeTitle, replySnippet, truncate } from "./lib/format";
import {
  approveSession,
  clearHistory,
  configureAgent,
  copyText,
  getAvailableAgents,
  getPalettePayload,
  getRecentHandoffs,
  hidePalette,
  listen,
  notifyResult,
  openSettingsWindow,
  readDroppedFiles,
  sendHandoff,
} from "./lib/tauri";
import type {
  Action,
  AgentMeta,
  AgentMetaStatus,
  ApprovalResult,
  Capture,
  ChatTurn,
  DroppedFile,
  GalleryAgent,
  HandoffActivity,
  HandoffPhase,
  LiveSession,
  PalettePayload,
  PaletteStep,
  SendOutcome,
} from "./lib/types";
import { WELCOME_STORAGE_KEY } from "./lib/types";

/** Chat messages are delivered verbatim via the `ask` action template — the
 *  palette never shows action buttons, so a static fallback keeps sends
 *  working even if the daemon ever returns an action list without `ask`. */
const ASK_ACTION: Action = {
  id: "ask",
  name: "Ask",
  description: "Analyze the provided context and tell me what you recommend.",
  prompt_template: "",
};

/** How often the visible sheet re-reads agent/process status from the daemon. */
export const STATUS_POLL_MS = 4000;

/** Output idle for this long and the handoff reads as "Wrapping up". */
const WRAPPING_IDLE_MS = 3000;

/** First available agent index in the flat picker list (0 when none are available). */
function firstSelectableIndex(list: PickerItem[]): number {
  const idx = list.findIndex((it) => it.agent.status.available);
  return idx >= 0 ? idx : 0;
}

/** Moves the picker highlight, skipping unavailable rows when possible. */
function moveHighlight(list: PickerItem[], current: number, delta: 1 | -1): number {
  if (list.length === 0) return 0;
  const anyAvailable = list.some((it) => it.agent.status.available);
  if (!anyAvailable) return (current + delta + list.length) % list.length;
  for (let offset = 1; offset <= list.length; offset++) {
    const next = (current + delta * offset + list.length) % list.length;
    if (list[next].agent.status.available) return next;
  }
  return current;
}

export default function App() {
  const [step, setStep] = useState<PaletteStep>("loading");
  const [payload, setPayload] = useState<PalettePayload | null>(null);
  const [outcome, setOutcome] = useState<SendOutcome | null>(null);
  const [errorText, setErrorText] = useState("");
  const [history, setHistory] = useState<SendOutcome[]>([]);
  const [fromHistory, setFromHistory] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  /** Session order of recently used agent ids, most recent first. */
  const [recentIds, setRecentIds] = useState<string[]>([]);
  /** Live-streamed agent output while sending. */
  const [live, setLive] = useState("");
  /** Byte length of the rendered prompt, straight from `handoff:started`. */
  const [promptBytes, setPromptBytes] = useState<number | null>(null);
  /** Wall-clock stamps behind the activity section. */
  const [sendStartedAt, setSendStartedAt] = useState<number | null>(null);
  const [firstChunkAt, setFirstChunkAt] = useState<number | null>(null);
  const [lastChunkAt, setLastChunkAt] = useState<number | null>(null);
  const sendStartedAtRef = useRef<number | null>(null);
  const firstChunkAtRef = useRef<number | null>(null);
  const promptBytesRef = useRef<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  /** Live chat thread (resumes the same session). */
  const [chatTurns, setChatTurns] = useState<ChatTurn[]>([]);
  const [chatSessionId, setChatSessionId] = useState<string | null>(null);
  const [chatSessionExplicit, setChatSessionExplicit] = useState(false);
  const [chatAgent, setChatAgent] = useState<AgentMeta | null>(null);
  const [chatPendingText, setChatPendingText] = useState<string | null>(null);
  /** Composer prefill — dropped context. */
  const [draft, setDraft] = useState("");
  const [draftSignal, setDraftSignal] = useState(0);
  /** First-run onboarding overlay. */
  const [showWelcome, setShowWelcome] = useState(false);
  const [welcomeGallery, setWelcomeGallery] = useState<GalleryAgent[]>([]);
  /** The keyboard cheat-sheet overlay. */
  const [showShortcuts, setShowShortcuts] = useState(false);
  /** Drag-over overlay + error state. */
  const [dragActive, setDragActive] = useState(false);
  const [dropError, setDropError] = useState("");
  /** In-flight approval state + note. */
  const [approvingAgent, setApprovingAgent] = useState<string | null>(null);
  const [approvalNote, setApprovalNote] = useState<{
    agentId: string;
    result: ApprovalResult;
  } | null>(null);

  /** Top filter input for the always-unfolded agent picker. */
  const filterInputRef = useRef<HTMLInputElement>(null);
  const [filterQuery, setFilterQuery] = useState("");
  const [highlight, setHighlight] = useState(0);

  /** Compact pop-up agent switcher inside the chat header. */
  const chatSwitcherRef = useRef<HTMLDivElement>(null);
  const [chatMenuOpen, setChatMenuOpen] = useState(false);

  const generationRef = useRef(0);
  const liveRef = useRef("");
  const handoffIdRef = useRef<string | null>(null);
  const stepRef = useRef<PaletteStep>(step);
  const welcomePendingRef = useRef<boolean | null>(null);
  const showWelcomeRef = useRef(false);
  const setWelcome = useCallback((v: boolean) => {
    showWelcomeRef.current = v;
    setShowWelcome(v);
  }, []);
  const dragDepthRef = useRef(0);

  const welcomePending = useCallback((): boolean => {
    if (welcomePendingRef.current === null) {
      let seen = false;
      try {
        seen = !!localStorage.getItem(WELCOME_STORAGE_KEY);
      } catch {
        /* storage unavailable — treat as seen */
      }
      welcomePendingRef.current = !seen;
    }
    return welcomePendingRef.current;
  }, []);

  useEffect(() => {
    stepRef.current = step;
  }, [step]);

  const openSettings = useCallback(() => {
    void openSettingsWindow().catch(() => undefined);
  }, []);

  const refresh = useCallback(async () => {
    generationRef.current += 1;
    try {
      const [p, h] = await Promise.all([getPalettePayload(), getRecentHandoffs()]);
      setPayload(p);
      setHistory(h);
      setRecentIds((cur) => {
        const fromHistory = [...new Set(h.map((o) => o.agent_id))];
        return [...fromHistory, ...cur.filter((id) => !fromHistory.includes(id))].slice(0, 5);
      });
      setDraft("");
      setDraftSignal((n) => n + 1);
      setFilterQuery("");
      setHighlight(0);
      setChatMenuOpen(false);
      setOutcome(null);
      setErrorText("");
      setFromHistory(false);
      setElapsed(0);
      setLive("");
      liveRef.current = "";
      setShowShortcuts(false);
      setDropError("");
      setApprovingAgent(null);
      setApprovalNote(null);
      setPromptBytes(null);
      setSendStartedAt(null);
      setFirstChunkAt(null);
      setLastChunkAt(null);
      setChatTurns([]);
      setChatSessionId(null);
      setChatAgent(null);
      setChatPendingText(null);
      setStep("home");
      requestAnimationFrame(() => filterInputRef.current?.focus());
    } catch (e) {
      setErrorText(String(e));
      setStep("error");
    }
  }, []);

  const dismissWelcome = useCallback(() => {
    try {
      localStorage.setItem(WELCOME_STORAGE_KEY, "1");
    } catch {
      /* storage unavailable */
    }
    welcomePendingRef.current = false;
    setWelcome(false);
    requestAnimationFrame(() => filterInputRef.current?.focus());
  }, [setWelcome]);

  const onPaletteOpen = useCallback(() => {
    if (showWelcomeRef.current) {
      dismissWelcome();
      void refresh();
    } else if (welcomePending()) {
      welcomePendingRef.current = false;
      setWelcome(true);
      getAvailableAgents()
        .then(setWelcomeGallery)
        .catch(() => setWelcomeGallery([]));
    } else {
      void refresh();
    }
  }, [refresh, welcomePending, dismissWelcome, setWelcome]);

  const openHistory = useCallback(async (handoffId?: string) => {
    try {
      const h = await getRecentHandoffs();
      setHistory(h);
      const entry = handoffId ? h.find((o) => o.id === handoffId) : undefined;
      if (entry) {
        setOutcome(entry);
        setFromHistory(true);
        setStep("done");
      } else {
        setFromHistory(false);
        setOutcome(null);
        setStep("history");
      }
    } catch (e) {
      setErrorText(String(e));
      setStep("error");
    }
  }, []);

  const hide = useCallback(() => {
    void hidePalette();
  }, []);

  // Click-outside closes the compact chat agent switcher popover.
  useEffect(() => {
    if (!chatMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (!chatSwitcherRef.current?.contains(e.target as Node)) {
        setChatMenuOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [chatMenuOpen]);

  // Live elapsed timer while a handoff is running.
  useEffect(() => {
    if (step !== "chat") {
      setElapsed(0);
      return;
    }
    const started = Date.now();
    const id = window.setInterval(() => {
      setElapsed(Math.floor((Date.now() - started) / 1000));
      setNow(Date.now());
    }, 500);
    return () => window.clearInterval(id);
  }, [step]);

  // Live process lights: surgical payload refresh while the palette is open.
  useEffect(() => {
    if (step !== "home" && step !== "chat") return;
    const ms =
      (window as unknown as { __handoverStatusPollMs?: number })
        .__handoverStatusPollMs ?? STATUS_POLL_MS;
    const id = window.setInterval(() => {
      void getPalettePayload()
        .then((p) => setPayload(p))
        .catch(() => undefined);
    }, ms);
    return () => window.clearInterval(id);
  }, [step]);

  const notify = useCallback((title: string, body: string) => {
    void notifyResult(title, body).catch(() => undefined);
  }, []);

  const handleApprove = useCallback(
    async (agent: AgentMeta, session: LiveSession, approve: boolean) => {
      setApprovingAgent(agent.id);
      setApprovalNote(null);
      try {
        const result = await approveSession(agent.id, session.session_id, approve);
        setApprovalNote({ agentId: agent.id, result });
      } catch (e) {
        setApprovalNote({
          agentId: agent.id,
          result: { verified: false, message: String(e) },
        });
      } finally {
        setApprovingAgent(null);
      }
    },
    []
  );

  // -------------------------------------------------------------------------
  // Drag & drop capture
  // -------------------------------------------------------------------------

  const captureFromDroppedFiles = useCallback(
    (files: DroppedFile[]): { capture?: Capture; error?: string } => {
      const usable = files.filter((f) => f.kind === "text" || f.kind === "image");
      const unusable = files.filter((f) => f.kind !== "text" && f.kind !== "image");
      if (usable.length === 0) {
        const reasons = unusable
          .map((f) => `${f.name}: ${f.note ?? "not readable"}`)
          .join("; ");
        return {
          error: `Couldn't read the dropped file${files.length > 1 ? "s" : ""} — ${reasons}`,
        };
      }
      const stamp = new Date().toISOString();
      if (usable.length === 1) {
        const f = usable[0];
        const capture: Capture = {
          id: "",
          timestamp: stamp,
          source: { type: "file" },
          content:
            f.kind === "image"
              ? { type: "image", text: null, path: f.path }
              : { type: "file", text: f.text ?? null, path: f.path },
          metadata: { dropped: "true", file: f.name, size: String(f.size) },
        };
        return { capture };
      }
      const items = usable.map((f) =>
        f.kind === "image" ? `Attached image: ${f.path}` : `File: ${f.path}\n\n${f.text ?? ""}`
      );
      return {
        capture: {
          id: "",
          timestamp: stamp,
          source: { type: "file" },
          content: { type: "mixed", text: null, items },
          metadata: { dropped: "true", files: String(usable.length) },
        },
      };
    },
    []
  );

  const installDroppedCapture = useCallback((capture: Capture) => {
    setDraft(captureSnippet(capture));
    setDraftSignal((n) => n + 1);
    setDropError("");
  }, []);

  const applyDroppedFiles = useCallback(
    async (paths: string[]) => {
      if (!paths.length) return;
      try {
        const files = await readDroppedFiles(paths);
        const { capture, error } = captureFromDroppedFiles(files);
        if (error) {
          setDropError(error);
          return;
        }
        if (capture) installDroppedCapture(capture);
      } catch (e) {
        setDropError(String(e));
      }
    },
    [captureFromDroppedFiles, installDroppedCapture]
  );

  const applyDroppedText = useCallback(
    (raw: string) => {
      const text = raw.trim();
      if (!text) return;
      installDroppedCapture({
        id: "",
        timestamp: new Date().toISOString(),
        source: { type: "manual", application: "Drag & drop" },
        content: { type: "text", text },
        metadata: { dropped: "true" },
      });
    },
    [installDroppedCapture]
  );

  const onDragEnter = useCallback((e: DragEvent) => {
    e.preventDefault();
    dragDepthRef.current += 1;
    setDragActive(true);
  }, []);
  const onDragOver = useCallback((e: DragEvent) => {
    e.preventDefault();
    setDragActive(true);
  }, []);
  const onDragLeave = useCallback((e: DragEvent) => {
    e.preventDefault();
    dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
    if (dragDepthRef.current === 0) setDragActive(false);
  }, []);
  const onDrop = useCallback(
    (e: DragEvent) => {
      e.preventDefault();
      dragDepthRef.current = 0;
      setDragActive(false);
      const text = e.dataTransfer?.getData("text/plain") ?? "";
      if (text.trim()) applyDroppedText(text);
    },
    [applyDroppedText]
  );

  useEffect(() => {
    const unlisteners: (() => void)[] = [];
    listen("palette:open", onPaletteOpen).then((fn) => unlisteners.push(fn));
    listen("palette:history", () => openHistory()).then((fn) => unlisteners.push(fn));
    listen("palette:handoff", (e) => openHistory(e.payload as string)).then((fn) =>
      unlisteners.push(fn)
    );
    listen<{ id: string; stream: string; chunk: string }>("handoff:output", (e) => {
      if (stepRef.current !== "chat") return;
      if (e.payload.id !== handoffIdRef.current) return;
      let text = liveRef.current + e.payload.chunk;
      if (text.length > 500_000) text = text.slice(-500_000);
      liveRef.current = text;
      setLive(text);
      const at = Date.now();
      setFirstChunkAt((prev) => (prev === null ? at : prev));
      setLastChunkAt(at);
      if (firstChunkAtRef.current === null) firstChunkAtRef.current = at;
    }).then((fn) => unlisteners.push(fn));
    listen<{ id: string; prompt_len: number }>("handoff:started", (e) => {
      handoffIdRef.current = e.payload.id;
      setPromptBytes(e.payload.prompt_len);
      promptBytesRef.current = e.payload.prompt_len;
    }).then((fn) => unlisteners.push(fn));
    listen("tauri://drag-enter", () => setDragActive(true)).then((fn) =>
      unlisteners.push(fn)
    );
    listen("tauri://drag-leave", () => setDragActive(false)).then((fn) =>
      unlisteners.push(fn)
    );
    listen<{ paths: string[] }>("tauri://drag-drop", (e) => {
      dragDepthRef.current = 0;
      setDragActive(false);
      void applyDroppedFiles(e.payload.paths ?? []);
    }).then((fn) => unlisteners.push(fn));
    refresh();
    return () => {
      unlisteners.forEach((fn) => fn());
    };
  }, [refresh, openHistory, onPaletteOpen, applyDroppedFiles]);

  // -------------------------------------------------------------------------
  // Agent grouping + filtering
  // -------------------------------------------------------------------------

  const orderedAgents = useMemo((): AgentMetaStatus[] => {
    if (!payload) return [];
    const rank = (a: AgentMetaStatus): number => {
      const ri = recentIds.indexOf(a.meta.id);
      if (ri >= 0) return ri;
      return 10;
    };
    return [...payload.agents].sort(
      (a, b) => rank(a) - rank(b) || a.meta.name.localeCompare(b.meta.name)
    );
  }, [payload, recentIds]);

  const sessionsByAgent = useMemo(() => {
    const m = new Map<string, LiveSession>();
    for (const s of payload?.sessions ?? []) {
      if (!m.has(s.agent_id)) m.set(s.agent_id, s);
    }
    return m;
  }, [payload]);

  /** Full grouped list (used by the chat switcher and as the filter base). */
  const allItems = useMemo((): PickerItem[] => {
    return orderedAgents.map((a) => ({
      agent: a,
      session: sessionsByAgent.get(a.meta.id) ?? null,
    }));
  }, [orderedAgents, sessionsByAgent]);

  /** Filtered grouped list for the always-unfolded home picker. */
  const pickerGroups = useMemo((): PickerGroup[] => {
    if (!payload) return [];
    const q = filterQuery.trim().toLowerCase();
    const running: PickerItem[] = [];
    const available: PickerItem[] = [];
    const missing: PickerItem[] = [];
    for (const a of orderedAgents) {
      if (q) {
        const hay = `${a.meta.name} ${a.meta.id} ${a.meta.description}`.toLowerCase();
        if (!hay.includes(q)) continue;
      }
      const session = sessionsByAgent.get(a.meta.id) ?? null;
      const runningNow = !!a.status.running || session?.activity === "working";
      if (runningNow) running.push({ agent: a, session });
      else if (a.status.available) available.push({ agent: a, session });
      else missing.push({ agent: a, session });
    }
    const groups: PickerGroup[] = [];
    if (running.length) groups.push({ label: "Running now", items: running });
    if (available.length) groups.push({ label: "Available", items: available });
    if (missing.length) groups.push({ label: "Not found", items: missing });
    return groups;
  }, [payload, orderedAgents, sessionsByAgent, filterQuery]);

  const pickerItems = useMemo(() => pickerGroups.flatMap((g) => g.items), [pickerGroups]);

  // -------------------------------------------------------------------------
  // Send (chat)
  // -------------------------------------------------------------------------

  const doSend = useCallback(
    async (agent: AgentMeta, capture: Capture, opts?: { sessionId?: string | null }) => {
      const gen = generationRef.current;
      const action = payload?.actions.find((a) => a.id === "ask") ?? ASK_ACTION;
      setStep("chat");
      setElapsed(0);
      liveRef.current = "";
      handoffIdRef.current = null;
      setLive("");
      const startedAt = Date.now();
      setSendStartedAt(startedAt);
      setPromptBytes(null);
      setFirstChunkAt(null);
      setLastChunkAt(null);
      sendStartedAtRef.current = startedAt;
      firstChunkAtRef.current = null;
      promptBytesRef.current = null;

      const measured = () => ({
        promptBytes: promptBytesRef.current,
        firstResponseMs:
          sendStartedAtRef.current !== null && firstChunkAtRef.current !== null
            ? Math.max(0, firstChunkAtRef.current - sendStartedAtRef.current)
            : null,
      });
      await new Promise<void>((r) => requestAnimationFrame(() => r()));

      const appendFailedTurn = (out: SendOutcome) => {
        setChatTurns((t) => [...t, { prompt: out.prompt, outcome: out, ...measured() }]);
        setChatPendingText(null);
        setOutcome(out);
        setStep("chat");
      };

      try {
        const res = await sendHandoff(action.id, agent.id, capture, opts?.sessionId ?? null);
        setRecentIds((cur) => [agent.id, ...cur.filter((id) => id !== agent.id)].slice(0, 5));
        if (gen !== generationRef.current) return;
        setOutcome(res);
        if (res.ok) {
          if (res.session_id) {
            setChatSessionId(res.session_id);
            setChatSessionExplicit(true);
          }
          setChatTurns((t) => [...t, { prompt: res.prompt, outcome: res, ...measured() }]);
          setChatPendingText(null);
          setStep("chat");
          const dur = res.receipt ? ` in ${fmtDuration(res.receipt.duration_ms)}` : "";
          const firstLine = replySnippet(res);
          const replyBit = firstLine ? ` - ${truncate(firstLine, 140)}` : "";
          notify("Handover", `${res.agent_name} completed${dur}${replyBit}`);
        } else {
          const msg = failureMessage(res);
          appendFailedTurn(res);
          notify("Handover — handoff failed", msg);
        }
      } catch (e) {
        const msg = String(e);
        if (gen === generationRef.current) {
          appendFailedTurn({
            id: `chat-${Date.now()}`,
            ok: false,
            agent_id: agent.id,
            agent_name: agent.name,
            action_id: action.id,
            capture,
            created_at: new Date().toISOString(),
            prompt: "",
            receipt: null,
            error: msg,
          });
        }
        notify("Handover — handoff failed", msg);
      }
    },
    [payload, notify]
  );

  const chooseAgent = useCallback((item: PickerItem) => {
    const { agent, session } = item;
    if (!agent.status.available) return;
    setChatMenuOpen(false);
    setFilterQuery("");
    setChatAgent(agent.meta);
    setChatSessionId(session?.session_id ?? null);
    setChatSessionExplicit(false);
    setChatTurns([]);
    setChatPendingText(null);
    setStep("chat");
  }, []);

  const enterChat = useCallback(() => {
    if (!outcome) return;
    const sameThread =
      chatSessionId !== null &&
      chatAgent?.id === outcome.agent_id &&
      outcome.session_id === chatSessionId;
    if (!sameThread) {
      setChatAgent({
        id: outcome.agent_id,
        name: outcome.agent_name,
        description: "",
        kind: "command",
      });
      setChatSessionId(outcome.session_id ?? null);
      setChatSessionExplicit(true);
      setChatTurns([{ prompt: outcome.prompt, outcome }]);
    }
    setChatMenuOpen(false);
    setChatPendingText(null);
    setStep("chat");
  }, [outcome, chatSessionId, chatAgent]);

  const sendChatMessage = useCallback(
    async (text: string) => {
      const t = text.trim();
      if (!t || !chatAgent) return;
      const capture: Capture = {
        id: "",
        timestamp: new Date().toISOString(),
        source: { type: "manual", application: "handover" },
        content: { type: "text", text: t },
        metadata: { chat: "true" },
      };
      setChatPendingText(t);
      await doSend(chatAgent, capture, {
        sessionId: chatSessionExplicit ? chatSessionId : null,
      });
    },
    [chatAgent, chatSessionId, chatSessionExplicit, doSend]
  );

  // -------------------------------------------------------------------------
  // Navigation / history
  // -------------------------------------------------------------------------

  const goHome = useCallback(() => {
    setStep("home");
    setChatAgent(null);
    setChatMenuOpen(false);
    setFilterQuery("");
    setHighlight(0);
    setFromHistory(false);
    requestAnimationFrame(() => filterInputRef.current?.focus());
  }, []);

  const goHistory = useCallback(() => {
    setChatMenuOpen(false);
    setStep("history");
    setFromHistory(false);
  }, []);

  const openHistoryEntry = useCallback((o: SendOutcome) => {
    setOutcome(o);
    setFromHistory(true);
    setStep("done");
  }, []);

  const retryFromHistory = useCallback((o: SendOutcome) => {
    setChatAgent({
      id: o.agent_id,
      name: o.agent_name,
      description: "",
      kind: "command",
    });
    setChatSessionId(o.session_id ?? null);
    setChatSessionExplicit(true);
    setChatTurns([]);
    setChatPendingText(null);
    setDraft(captureSnippet(o.capture ?? null) ?? "");
    setDraftSignal((n) => n + 1);
    setStep("chat");
  }, []);

  const resendWithAnotherFromHistory = useCallback(
    (o: SendOutcome) => {
      setDraft(captureSnippet(o.capture ?? null) ?? "");
      setDraftSignal((n) => n + 1);
      goHome();
    },
    [goHome]
  );

  const clearAllHistory = useCallback(async () => {
    try {
      await clearHistory();
      setHistory([]);
    } catch (e) {
      setErrorText(String(e));
      setStep("error");
    }
  }, []);

  // Clamp highlight and keep it on an available row whenever the list changes.
  useEffect(() => {
    const len = pickerItems.length;
    if (len === 0) {
      if (highlight !== 0) setHighlight(0);
    } else if (highlight >= len || !pickerItems[highlight]?.agent.status.available) {
      const next = firstSelectableIndex(pickerItems);
      if (next !== highlight) setHighlight(next);
    }
  }, [pickerItems, highlight]);

  const onKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ",") {
        e.preventDefault();
        openSettings();
        return;
      }
      if (showWelcome) {
        if (e.key === "Escape") hide();
        else if (e.key === "Enter") dismissWelcome();
        return;
      }
      if (showShortcuts) {
        if (e.key === "Escape") setShowShortcuts(false);
        return;
      }
      const target = e.target as HTMLElement | null;
      if (e.key === "?") {
        const typing = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA";
        if (!typing) {
          e.preventDefault();
          setShowShortcuts(true);
          return;
        }
      }
      if (e.key === "Escape") {
        if (step === "home" && filterQuery) {
          e.preventDefault();
          setFilterQuery("");
          setHighlight(0);
        } else if (step === "chat" && chatMenuOpen) {
          e.preventDefault();
          setChatMenuOpen(false);
        } else if (step === "chat") {
          e.preventDefault();
          goHome();
        } else if (step === "done" && fromHistory) {
          e.preventDefault();
          setStep("history");
        } else {
          hide();
        }
        return;
      }

      if (target?.tagName === "TEXTAREA") return;

      if (step === "home") {
        const list = pickerItems;
        if (list.length === 0) return;
        if (e.key === "ArrowDown") {
          e.preventDefault();
          setHighlight((s) => moveHighlight(list, s, 1));
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          setHighlight((s) => moveHighlight(list, s, -1));
        } else if (e.key === "Enter" && target?.tagName !== "BUTTON") {
          e.preventDefault();
          chooseAgent(list[highlight]);
        }
      }
    },
    [
      step,
      filterQuery,
      chatMenuOpen,
      pickerItems,
      highlight,
      chooseAgent,
      goHome,
      hide,
      openSettings,
      fromHistory,
      showWelcome,
      showShortcuts,
      dismissWelcome,
    ]
  );

  useEffect(() => {
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onKeyDown]);

  // -------------------------------------------------------------------------
  // Actions on results / history
  // -------------------------------------------------------------------------

  const copyText_ = useCallback((text: string) => {
    if (text) void copyText(text);
  }, []);

  const copyPrompt = useCallback(() => {
    if (outcome?.prompt) copyText_(outcome.prompt);
  }, [outcome, copyText_]);

  const copyReply = useCallback(() => {
    if (outcome?.receipt?.stdout) copyText_(outcome.receipt.stdout);
  }, [outcome, copyText_]);

  // -------------------------------------------------------------------------
  // Render
  // -------------------------------------------------------------------------

  const activity: HandoffActivity | null = useMemo(() => {
    const sending = chatPendingText !== null;
    if (!sending && chatTurns.length === 0) return null;
    const phase: HandoffPhase = !sending
      ? "done"
      : lastChunkAt === null
        ? "sending"
        : now - lastChunkAt > WRAPPING_IDLE_MS
          ? "wrapping"
          : "responding";
    const output = sending
      ? live
      : [outcome?.receipt?.stdout ?? "", outcome?.receipt?.stderr ?? ""]
          .filter(Boolean)
          .join("\n");
    return {
      phase,
      promptBytes,
      firstResponseMs:
        sendStartedAt !== null && firstChunkAt !== null
          ? Math.max(0, firstChunkAt - sendStartedAt)
          : null,
      outputBytes: byteLength(output),
      output,
    };
  }, [
    chatPendingText,
    chatTurns.length,
    live,
    outcome,
    lastChunkAt,
    now,
    promptBytes,
    sendStartedAt,
    firstChunkAt,
  ]);

  const chatLiveSession = chatAgent ? sessionsByAgent.get(chatAgent.id) ?? null : null;
  const chatAgentStatus = chatAgent
    ? payload?.agents.find((a) => a.meta.id === chatAgent.id)
    : undefined;
  const chatIsRunning =
    !!chatAgentStatus?.status.running || chatLiveSession?.activity === "working";

  const paletteRef = useRef<HTMLDivElement>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const sizeMode: "home" | "panel" =
    (step === "home" || step === "loading") && !dragActive && !showWelcome && !showShortcuts
      ? "home"
      : "panel";
  usePaletteSize(paletteRef, cardRef, sizeMode);

  return (
    <div
      ref={paletteRef}
      className={`palette${dragActive ? " dropping" : ""}${sizeMode === "home" ? " home-mode" : ""}`}
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      <div className="palette-stack">
        <div className="palette-card" ref={cardRef}>
          {dragActive && (
            <div className="drop-overlay" data-testid="drop-overlay">
              <div className="drop-card">
                <Icon name="file" size={22} />
                <span className="drop-title">Drop to capture</span>
                <span className="drop-sub">Files and text pre-fill the chat</span>
              </div>
            </div>
          )}
          {showWelcome && (
            <WelcomeOverlay
              shortcut={payload?.shortcut ?? "CmdOrCtrl+Shift+A"}
              gallery={welcomeGallery}
              onAddAgent={(a) => {
                setWelcomeGallery((g) =>
                  g.map((x) => (x.id === a.id ? { ...x, configured: true } : x))
                );
                void configureAgent(a.id, null).catch(() =>
                  setWelcomeGallery((g) =>
                    g.map((x) => (x.id === a.id ? { ...x, configured: false } : x))
                  )
                );
              }}
              onDismiss={dismissWelcome}
            />
          )}
          {showShortcuts && (
            <ShortcutSheet
              shortcut={payload?.shortcut ?? "CmdOrCtrl+Shift+A"}
              onClose={() => setShowShortcuts(false)}
            />
          )}

          {/* Unified Tahoe top bar */}
          <header className="palette-header" onDoubleClick={hide} data-tauri-drag-region>
            {(step === "home" || step === "loading") && (
              <>
                <span className="logo" data-tauri-drag-region>
                  <HandoverMark size={16} />
                </span>
                <input
                  ref={filterInputRef}
                  type="text"
                  role="combobox"
                  aria-autocomplete="list"
                  aria-expanded={step === "home" && pickerItems.length > 0}
                  aria-controls="agent-picker-list"
                  aria-activedescendant={
                    step === "home" && pickerItems[highlight]
                      ? `agent-option-${pickerItems[highlight].agent.meta.id}`
                      : undefined
                  }
                  className="palette-filter"
                  data-testid="agent-filter"
                  aria-label="Filter agents"
                  placeholder="Choose an agent or type to filter…"
                  spellCheck={false}
                  autoComplete="off"
                  value={filterQuery}
                  onChange={(e) => {
                    setFilterQuery(e.target.value);
                    setHighlight(0);
                  }}
                />
                {filterQuery && (
                  <button
                    type="button"
                    className="icon-btn filter-clear"
                    aria-label="Clear filter"
                    onClick={() => {
                      setFilterQuery("");
                      setHighlight(0);
                      filterInputRef.current?.focus();
                    }}
                  >
                    <Icon name="close" size={12} />
                  </button>
                )}
              </>
            )}

            {step === "chat" && chatAgent && (
              <>
                <button
                  type="button"
                  className="icon-btn"
                  aria-label="All agents"
                  title="All agents (Esc)"
                  onClick={goHome}
                >
                  <Icon name="back" size={15} />
                </button>
                <div className="chat-switcher" ref={chatSwitcherRef}>
                  <button
                    type="button"
                    className="chat-agent-switcher"
                    data-testid="chat-agent-switcher"
                    aria-haspopup="menu"
                    aria-expanded={chatMenuOpen}
                    aria-label={`Agent: ${chatAgent.name}${chatIsRunning ? " (running)" : " (idle)"}`}
                    onClick={() => setChatMenuOpen((v) => !v)}
                  >
                    <span className="chat-switcher-logo">
                      <AgentLogo id={chatAgent.id} size={16} />
                    </span>
                    <span className="chat-switcher-name">{chatAgent.name}</span>
                    <span
                      className={`status-dot ${chatIsRunning ? "ok" : "idle"}`}
                      aria-hidden="true"
                    />
                    <Icon name="chevronsUpDown" size={11} className="chat-switcher-chev" />
                  </button>
                  {chatMenuOpen && (
                    <div
                      className="chat-switcher-menu"
                      role="menu"
                      aria-label="Switch agent"
                    >
                      {allItems.map((it) => {
                        const available = it.agent.status.available;
                        const active = it.agent.meta.id === chatAgent.id;
                        const running = isAgentRunning(it);
                        return (
                          <button
                            key={it.agent.meta.id}
                            type="button"
                            role="menuitem"
                            disabled={!available}
                            className={`chat-switcher-item${active ? " active" : ""}`}
                            onClick={() => chooseAgent(it)}
                          >
                            <span className="chat-switcher-logo">
                              <AgentLogo id={it.agent.meta.id} size={15} />
                            </span>
                            <span className="chat-switcher-item-name">
                              {it.agent.meta.name}
                            </span>
                            <span
                              className={`status-dot ${
                                !available ? "off" : running ? "ok" : "idle"
                              }`}
                              aria-hidden="true"
                            />
                            {active && <Icon name="check" size={12} />}
                          </button>
                        );
                      })}
                    </div>
                  )}
                </div>
              </>
            )}

            {(step === "history" || step === "done" || step === "error") && (
              <>
                <button
                  type="button"
                  className="icon-btn"
                  aria-label="Back"
                  title="Back"
                  onClick={() => {
                    if (step === "done" && fromHistory) setStep("history");
                    else goHome();
                  }}
                >
                  <Icon name="back" size={15} />
                </button>
                <span className="header-view-title" data-tauri-drag-region>
                  {step === "history"
                    ? "Recent handoffs"
                    : step === "done" && outcome
                      ? outcomeTitle(outcome)
                      : "Error"}
                </span>
              </>
            )}

            <div className="header-tools">
              <button
                type="button"
                className={`tool-btn${step === "history" || step === "done" ? " active" : ""}`}
                title="Recent handoffs"
                aria-label="Recent handoffs"
                onClick={step === "history" ? goHome : goHistory}
              >
                <Icon name="history" size={14} />
              </button>
              <button
                type="button"
                className="tool-btn"
                title="Keyboard shortcuts (?)"
                aria-label="Keyboard shortcuts"
                onClick={() => setShowShortcuts(true)}
              >
                <Icon name="keyboard" size={14} />
              </button>
              <button
                type="button"
                className="tool-btn"
                title="Settings (⌘,)"
                aria-label="Open settings"
                onClick={openSettings}
              >
                <Icon name="settings" size={14} />
              </button>
            </div>
          </header>

          <div className="palette-body">
            {dropError && (
              <div className="drop-error" role="alert" data-testid="drop-error">
                <Icon name="warning" size={12} />
                <span>{dropError}</span>
                <button
                  type="button"
                  className="icon-btn"
                  aria-label="Dismiss"
                  onClick={() => setDropError("")}
                >
                  <Icon name="close" size={11} />
                </button>
              </div>
            )}

            {step === "loading" && (
              <div className="status-body">
                <div className="spinner" />
                <p>Loading…</p>
              </div>
            )}

            {step === "home" && payload && (
              <div key={`home-${generationRef.current}`} className="step-in home-pane">
                {draft && (
                  <div className="captured-line" data-testid="captured-line">
                    <Icon name="clipboard" size={12} />
                    <span className="captured-text">
                      Will start the chat with: {truncate(draft, 80)}
                    </span>
                    <button
                      type="button"
                      className="icon-btn"
                      aria-label="Clear captured context"
                      onClick={() => {
                        setDraft("");
                        setDraftSignal((n) => n + 1);
                      }}
                    >
                      <Icon name="close" size={11} />
                    </button>
                  </div>
                )}

                {payload.agents.length === 0 ? (
                  <div className="empty-state" data-testid="empty-state">
                    <span className="status-icon warn" aria-hidden="true">
                      <Icon name="agentLocal" size={20} />
                    </span>
                    <h2>No agents are available yet.</h2>
                    <p>
                      Running agents (Claude Code, Hermes, Codex, …) appear here
                      automatically.
                    </p>
                    <div className="status-actions">
                      <button type="button" className="btn primary" onClick={openSettings}>
                        <Icon name="settings" size={13} />
                        Open Settings to add an agent
                      </button>
                    </div>
                  </div>
                ) : (
                  <AgentPicker
                    groups={pickerGroups}
                    filterQuery={filterQuery}
                    highlight={highlight}
                    onHighlight={setHighlight}
                    onChoose={chooseAgent}
                    onClearFilter={() => {
                      setFilterQuery("");
                      setHighlight(0);
                      filterInputRef.current?.focus();
                    }}
                    onOpenSettings={openSettings}
                  />
                )}
              </div>
            )}

            {step === "chat" && chatAgent && (
              <ChatView
                agent={chatAgent}
                sessionId={chatSessionId}
                session={chatLiveSession}
                turns={chatTurns}
                sendingText={chatPendingText}
                elapsed={elapsed}
                activity={activity}
                draft={draft}
                draftSignal={draftSignal}
                onApprove={
                  chatLiveSession?.blocked
                    ? (approve) => void handleApprove(chatAgent, chatLiveSession, approve)
                    : undefined
                }
                approving={approvingAgent === chatAgent.id}
                approvalNote={
                  approvalNote?.agentId === chatAgent.id ? approvalNote.result : null
                }
                onSend={(t) => void sendChatMessage(t)}
              />
            )}

            {step === "done" && outcome && (
              <ResultPanel
                outcome={outcome}
                fromHistory={fromHistory}
                onBack={() => setStep("history")}
                onCopyReply={copyReply}
                onCopyPrompt={copyPrompt}
                onChat={enterChat}
                onRepeatWithAnotherAgent={() =>
                  outcome && resendWithAnotherFromHistory(outcome)
                }
                onClose={hide}
              />
            )}

            {step === "history" && (
              <div key="history" className="step-in" style={{ display: "contents" }}>
                <HistoryList
                  history={history}
                  onOpen={openHistoryEntry}
                  onRetry={retryFromHistory}
                  onCopyResult={(o) => copyText_(replySnippet(o))}
                  onResendWithAnother={resendWithAnotherFromHistory}
                  onClear={() => void clearAllHistory()}
                />
              </div>
            )}

            {step === "error" && (
              <ErrorState
                message={errorText}
                canRetry={false}
                onRetry={() => undefined}
                onCopyPrompt={copyPrompt}
                onClose={hide}
              />
            )}
          </div>

          <footer className="palette-footer">
            <span className="footer-hint">
              {step === "history" ? (
                <>
                  <span className="hint-item">
                    <kbd>↵</kbd> Open
                  </span>
                  <span className="hint-item">
                    <kbd>esc</kbd> Close
                  </span>
                </>
              ) : step === "chat" ? (
                <>
                  <span className="hint-item">
                    <kbd>↵</kbd> Send
                  </span>
                  <span className="hint-item">
                    <kbd>⇧↵</kbd> Newline
                  </span>
                  <span className="hint-item">
                    <kbd>esc</kbd> Agents
                  </span>
                </>
              ) : step === "home" ? (
                <>
                  <span className="hint-item">
                    <kbd>↑↓</kbd> Select
                  </span>
                  <span className="hint-item">
                    <kbd>↵</kbd> Chat
                  </span>
                  <span className="hint-item">
                    <kbd>esc</kbd> {filterQuery ? "Clear" : "Close"}
                  </span>
                </>
              ) : (
                <span className="hint-item">
                  <kbd>esc</kbd> Close
                </span>
              )}
            </span>
          </footer>
        </div>
      </div>
    </div>
  );
}
