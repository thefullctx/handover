import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import { HandoverMark, Icon } from "./Icons";
import AgentDropdown, { type DdGroup, type DdItem } from "./components/AgentDropdown";
import ChatView from "./components/ChatView";
import ErrorState from "./components/ErrorState";
import HistoryList from "./components/HistoryList";
import ResultPanel from "./components/ResultPanel";
import ShortcutSheet from "./components/ShortcutSheet";
import WelcomeOverlay from "./components/WelcomeOverlay";
import { captureSnippet, byteLength, failureMessage, fmtDuration, formatShortcut, replySnippet, truncate } from "./lib/format";
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

/** How often the visible sheet re-reads agent/process status from the daemon
 *  (process lights stay live while you work in another terminal). Exported
 *  so tests can drive the clock precisely. */
export const STATUS_POLL_MS = 4000;

/** Output idle for this long and the handoff reads as "Wrapping up". The
 *  phase reverts to "Agent responding" the instant output resumes, so a slow
 *  agent that pauses mid-thought never looks finished. */
const WRAPPING_IDLE_MS = 3000;

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
  /** Byte length of the rendered prompt, straight from `handoff:started` —
   *  a real measurement from Rust, never re-derived in the UI. */
  const [promptBytes, setPromptBytes] = useState<number | null>(null);
  /** Wall-clock stamps behind the activity section: when the send began, and
   *  when agent output first arrived / last arrived. */
  const [sendStartedAt, setSendStartedAt] = useState<number | null>(null);
  const [firstChunkAt, setFirstChunkAt] = useState<number | null>(null);
  const [lastChunkAt, setLastChunkAt] = useState<number | null>(null);
  /** Ticks while the chat is open so the elapsed timer AND the phase
   *  derivation (idle > WRAPPING_IDLE_MS) stay live. */
  const [now, setNow] = useState(() => Date.now());
  /** Live chat thread (a conversation, not one reply) — turns append in
   *  place and every message explicitly resumes the same session. */
  const [chatTurns, setChatTurns] = useState<ChatTurn[]>([]);
  /** The session being resumed — seeded from the agent's freshest live
   *  session, updated if the agent rotates sessions mid-chat. */
  const [chatSessionId, setChatSessionId] = useState<string | null>(null);
  /** True once the thread is anchored to a specific session (adopted from a
   *  completed send, or seeded from history). False on a freshly picked
   *  thread: the first message sends with no session so the daemon resolves
   *  freshest-first itself — an owned live session then falls back to a
   *  fresh send instead of failing. */
  const [chatSessionExplicit, setChatSessionExplicit] = useState(false);
  const [chatAgent, setChatAgent] = useState<AgentMeta | null>(null);
  /** The message currently in flight (in-flight user bubble in the thread). */
  const [chatPendingText, setChatPendingText] = useState<string | null>(null);
  /** Composer prefill — captured clipboard / dropped context. Re-applied to
   *  a fresh chat whenever `draftSignal` bumps. */
  const [draft, setDraft] = useState("");
  const [draftSignal, setDraftSignal] = useState(0);
  /** First-run onboarding overlay (teach by doing — dismiss is the shortcut). */
  const [showWelcome, setShowWelcome] = useState(false);
  /** Catalog for the welcome's "pick your assistants" step (loaded lazily). */
  const [welcomeGallery, setWelcomeGallery] = useState<GalleryAgent[]>([]);
  /** The keyboard cheat-sheet overlay (`?` or the footer button). */
  const [showShortcuts, setShowShortcuts] = useState(false);
  /** True while a drag hovers the palette (drop overlay visible). */
  const [dragActive, setDragActive] = useState(false);
  /** Plain-language error when a drop couldn't be read. */
  const [dropError, setDropError] = useState("");
  /** Agent id whose approval decision is in flight (buttons disabled). */
  const [approvingAgent, setApprovingAgent] = useState<string | null>(null);
  /** Result of the last approval, per agent (verified / fail-soft message). */
  const [approvalNote, setApprovalNote] = useState<{
    agentId: string;
    result: ApprovalResult;
  } | null>(null);

  /** The agent dropdown's trigger button (keyboard focus target). */
  const ddTriggerRef = useRef<HTMLButtonElement>(null);
  /** Dropdown state: open/closed, the highlighted option (flat index), and
   *  a bump counter so reopening always lands the highlight on the selected
   *  (or first) entry. */
  const [ddOpen, setDdOpen] = useState(false);
  const [ddHighlight, setDdHighlight] = useState(0);
  const [ddOpenTick, setDdOpenTick] = useState(0);
  const generationRef = useRef(0);
  const liveRef = useRef("");
  const handoffIdRef = useRef<string | null>(null);
  const stepRef = useRef<PaletteStep>(step);
  /** Whether the first-run welcome still needs presenting (lazy-loaded). */
  const welcomePendingRef = useRef<boolean | null>(null);
  /** Mirrors `showWelcome` synchronously so the palette:open listener never
   *  sees a stale value (a fast second press must be able to dismiss). */
  const showWelcomeRef = useRef(false);
  const setWelcome = useCallback((v: boolean) => {
    showWelcomeRef.current = v;
    setShowWelcome(v);
  }, []);
  /** dragenter/dragleave depth — dragleave fires per child boundary. */
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
      // Seed \"recently used\" ordering from the session history.
      setRecentIds((cur) => {
        const fromHistory = [...new Set(h.map((o) => o.agent_id))];
        return [...fromHistory, ...cur.filter((id) => !fromHistory.includes(id))].slice(0, 5);
      });
      // The palette opens clean — clipboard capture was removed, so the chat
      // composer starts empty (dropped files/text still seed it).
      setDraft("");
      setDraftSignal((n) => n + 1);
      // The palette reopens on a fresh picker — closed, no stale highlight,
      // keyboard focus back on the trigger.
      setDdOpen(false);
      setDdHighlight(0);
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
      requestAnimationFrame(() => ddTriggerRef.current?.focus());
    } catch (e) {
      setErrorText(String(e));
      setStep("error");
    }
  }, []);

  /** Dismisses the welcome for good (Start button, Enter, or a second press
   *  of the hotkey via onPaletteOpen). The next launch stays quiet. */
  const dismissWelcome = useCallback(() => {
    try {
      localStorage.setItem(WELCOME_STORAGE_KEY, "1");
    } catch {
      /* storage unavailable — the flag simply won't persist */
    }
    welcomePendingRef.current = false;
    setWelcome(false);
    requestAnimationFrame(() => ddTriggerRef.current?.focus());
  }, [setWelcome]);

  /** The palette's primary entry point (the global hotkey).
   *  - First ever run: present the welcome overlay (teach by doing).
   *  - Welcome visible: the press IS the dismissal — hide it, then refresh.
   *  - Otherwise: refresh the sheet. */
  const onPaletteOpen = useCallback(() => {
    if (showWelcomeRef.current) {
      dismissWelcome();
      void refresh();
    } else if (welcomePending()) {
      welcomePendingRef.current = false;
      setWelcome(true);
      // Load the catalog for the pick-your-assistants step (best effort —
      // onboarding must never fail because the gallery didn't load).
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

  // Live elapsed timer while a handoff is running. The same tick also drives
  // `now`, which is what lets the activity phase notice that output has gone
  // quiet (wrapping up) without its own timer.
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

  // Live process lights: while the sheet is visible, re-read the daemon's
  // agent/session status so a dot flips red→green the moment you start (or
  // quit) an agent elsewhere — no hide/reopen cycle needed. This merge is
  // deliberately SURGICAL: only `payload` updates, so the open dropdown, its
  // highlight, the chat thread and the composer draft all survive untouched.
  // Tests shorten the interval via `__handoverStatusPollMs` (real timers).
  useEffect(() => {
    if (step !== "home" && step !== "chat") return;
    const ms =
      (window as unknown as { __handoverStatusPollMs?: number })
        .__handoverStatusPollMs ?? STATUS_POLL_MS;
    const id = window.setInterval(() => {
      void getPalettePayload()
        .then((p) => setPayload(p))
        .catch(() => undefined); // one missed tick must never surface as an error
    }, ms);
    return () => window.clearInterval(id);
  }, [step]);

  const notify = useCallback((title: string, body: string) => {
    void notifyResult(title, body).catch(() => undefined);
  }, []);

  /** Approve/deny a blocked session. Only an explicit button press injects;
   *  the daemon re-checks + verifies (fail soft — never a false success). */
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
  // Drag & drop capture — dropped text/files pre-fill the chat composer
  // -------------------------------------------------------------------------

  /** Builds a Capture from the files `read_dropped_files` returned, or a
   *  plain-language reason why nothing could be used. */
  const captureFromDroppedFiles = useCallback(
    (files: DroppedFile[]): { capture?: Capture; error?: string } => {
      const usable = files.filter((f) => f.kind === "text" || f.kind === "image");
      const unusable = files.filter((f) => f.kind !== "text" && f.kind !== "image");
      if (usable.length === 0) {
        const reasons = unusable
          .map((f) => `${f.name}: ${f.note ?? "not readable"}`)
          .join("; ");
        return { error: `Couldn't read the dropped file${files.length > 1 ? "s" : ""} — ${reasons}` };
      }
      const now = new Date().toISOString();
      if (usable.length === 1) {
        const f = usable[0];
        const capture: Capture = {
          id: "",
          timestamp: now,
          source: { type: "file" },
          content:
            f.kind === "image"
              ? { type: "image", text: null, path: f.path }
              : { type: "file", text: f.text ?? null, path: f.path },
          metadata: { dropped: "true", file: f.name, size: String(f.size) },
        };
        return { capture };
      }
      // Multiple usable files — hand them off as one mixed capture. The
      // prompt renderer joins `items` with path headers.
      const items = usable.map((f) =>
        f.kind === "image" ? `Attached image: ${f.path}` : `File: ${f.path}\n\n${f.text ?? ""}`
      );
      return {
        capture: {
          id: "",
          timestamp: now,
          source: { type: "file" },
          content: { type: "mixed", text: null, items },
          metadata: { dropped: "true", files: String(usable.length) },
        },
      };
    },
    []
  );

  /** Installs dropped context as the composer draft (shown on home, pre-fills
   *  the chat composer for the next agent chosen). */
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

  /** Text dropped directly (DOM drag — webviews allow text drags). */
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

  // DOM handlers for text drags; file drags arrive via `tauri://drag-*`.
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
      // Files are handled by the native tauri://drag-drop event (the webview
      // won't expose dataTransfer.files for external drags on macOS).
      const text = e.dataTransfer?.getData("text/plain") ?? "";
      if (text.trim()) applyDroppedText(text);
    },
    [applyDroppedText]
  );

  // Event listeners live here (after all handlers they reference).
  useEffect(() => {
    const unlisteners: (() => void)[] = [];
    listen("palette:open", onPaletteOpen).then((fn) => unlisteners.push(fn));
    listen("palette:history", () => openHistory()).then((fn) => unlisteners.push(fn));
    listen("palette:handoff", (e) => openHistory(e.payload as string)).then((fn) =>
      unlisteners.push(fn)
    );
    listen<{ id: string; stream: string; chunk: string }>("handoff:output", (e) => {
      if (stepRef.current !== "chat") return;
      // Ignore chunks that do not belong to the current handoff (dismissed
      // palette, retried send, or a second window). The comparison is
      // deliberately unguarded: before this send's handoff:started arrives
      // `handoffIdRef.current` is null, and `id !== null` rejects every
      // chunk — including stragglers still streaming from a previous,
      // still-running handoff. Guarding with `handoffIdRef.current &&`
      // would disable the filter for exactly that window and splice old
      // output into the new turn (and stamp its time-to-first-response).
      if (e.payload.id !== handoffIdRef.current) return;
      let text = liveRef.current + e.payload.chunk;
      if (text.length > 500_000) text = text.slice(-500_000);
      liveRef.current = text;
      setLive(text);
      // Activity stamps: first output drives "time to first response", the
      // latest one drives the wrapping-up phase. Both are real observations
      // of the stream, not estimates.
      const at = Date.now();
      setFirstChunkAt((prev) => (prev === null ? at : prev));
      setLastChunkAt(at);
    }).then((fn) => unlisteners.push(fn));
    // The daemon emits the stable handoff id + rendered prompt size as soon
    // as the send starts. The id correlates streamed `handoff:output` chunks
    // with this send (see above); prompt_len is a real measurement for stats.
    listen<{ id: string; prompt_len: number }>("handoff:started", (e) => {
      handoffIdRef.current = e.payload.id;
      // The rendered prompt's byte size, measured in Rust. This is the "prompt
      // size" stat in the activity section.
      setPromptBytes(e.payload.prompt_len);
    }).then((fn) => unlisteners.push(fn));
    // External file drags: WKWebView blocks dataTransfer.files, so Tauri
    // forwards the drop paths through these native events. Text drags come
    // through the DOM handlers on the root instead.
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


  /** The agent order for BOTH rendering and keyboard navigation — one list,
   *  so the highlighted row is always the one Enter chats with. */
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

  /** Freshest live session per agent (sessions arrive freshest-first from the
   *  daemon) — drives the Running-now grouping, status chips and the session
   *  each chat resumes. */
  const sessionsByAgent = useMemo(() => {
    const m = new Map<string, LiveSession>();
    for (const s of payload?.sessions ?? []) {
      if (!m.has(s.agent_id)) m.set(s.agent_id, s);
    }
    return m;
  }, [payload]);

  /** Grouped dropdown entries: Running now → Available → Not found. The
   *  trigger names the agent being chatted with; the menu is the picker.
   *  Group headers are decorative; agent options are what the keyboard
   *  navigates (flat index across groups). */
  const ddGroups = useMemo((): DdGroup[] => {
    if (!payload) return [];
    const running: DdItem[] = [];
    const available: DdItem[] = [];
    const missing: DdItem[] = [];
    for (const a of orderedAgents) {
      const session = sessionsByAgent.get(a.meta.id);
      // "Running now" means the agent's process is alive right now (Hermes
      // with its TUI open, omp in a terminal) or its session is actively
      // writing — not merely that a session file exists. A quiet session on
      // an idle agent (e.g. Codex with no process) sits under Available with
      // its honest "last" chip instead.
      const runningNow = !!a.status.running || session?.activity === "working";
      if (runningNow) running.push({ agent: a, session: session ?? null });
      else if (a.status.available) available.push({ agent: a, session: session ?? null });
      else missing.push({ agent: a, session: session ?? null });
    }
    const groups: DdGroup[] = [];
    if (running.length) groups.push({ label: "Running now", items: running });
    if (available.length) groups.push({ label: "Available", items: available });
    if (missing.length) groups.push({ label: "Not found", items: missing });
    return groups;
  }, [payload, orderedAgents, sessionsByAgent]);

  /** Flat option order for keyboard navigation (visual order). */
  const ddItems = useMemo(() => ddGroups.flatMap((g) => g.items), [ddGroups]);

  // -------------------------------------------------------------------------
  // Send (chat only — the palette has no action buttons)
  // -------------------------------------------------------------------------

  const doSend = useCallback(
    async (agent: AgentMeta, capture: Capture, opts?: { sessionId?: string | null }) => {
      const gen = generationRef.current;
      const action =
        payload?.actions.find((a) => a.id === "ask") ?? ASK_ACTION;
      setStep("chat");
      setElapsed(0);
      liveRef.current = "";
      handoffIdRef.current = null;
      setLive("");
      // Fresh activity measurements for this handoff. promptBytes is reset
      // too — until this send's handoff:started lands, the previous
      // handoff's prompt size would otherwise be attributed to this one.
      setSendStartedAt(Date.now());
      setPromptBytes(null);
      setFirstChunkAt(null);
      setLastChunkAt(null);
      await new Promise<void>((r) => requestAnimationFrame(() => r()));

      /** A failed exchange still lands in the thread — the user sees the
       *  error in place instead of being bounced out of the conversation. */
      const appendFailedTurn = (outcome: SendOutcome) => {
        setChatTurns((t) => [...t, { prompt: outcome.prompt, outcome }]);
        setChatPendingText(null);
        setOutcome(outcome);
        setStep("chat");
      };

      try {
        const res = await sendHandoff(action.id, agent.id, capture, opts?.sessionId ?? null);
        setRecentIds((cur) => [agent.id, ...cur.filter((id) => id !== agent.id)].slice(0, 5));
        if (gen !== generationRef.current) return; // stale (palette reopened)
        setOutcome(res);
        if (res.ok) {
          // Record the session the reply actually landed in (the agent may
          // have rotated sessions on resume, or fallen back to fresh).
          if (res.session_id) {
            setChatSessionId(res.session_id);
            setChatSessionExplicit(true);
          }
          setChatTurns((t) => [...t, { prompt: res.prompt, outcome: res }]);
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

  /** Opens a chat with a dropdown entry — the thread lives directly beneath
   *  the trigger; choosing an agent never navigates away. */
  const chooseFromDropdown = useCallback(
    (item: DdItem) => {
      const { agent, session } = item;
      if (!agent.status.available) return;
      setDdOpen(false);
      setChatAgent(agent.meta);
      setChatSessionId(session?.session_id ?? null);
      // New thread, nothing anchored yet: the first message lets the daemon
      // resolve the session (freshest-first with owned-session fallback).
      setChatSessionExplicit(false);
      // The composer keeps its captured/dropped draft — the context follows
      // the user into whichever conversation they start.
      setChatTurns([]);
      setChatPendingText(null);
    },
    []
  );

  /** Open the live chat thread for the current result. Continuing the same
   *  session keeps the thread (and every message resumes that session); a
   *  different result seeds a fresh thread from it. */
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
    setChatPendingText(null);
    setStep("chat");
  }, [outcome, chatSessionId, chatAgent]);

  /** Send a chat message. The first message of a fresh thread carries no
   *  session (the daemon resolves freshest-first, with owned-session
   *  fallback); once anchored, every message explicitly resumes the SAME
   *  session — never left to freshest-resolution. Stays in the thread,
   *  streams inline. */
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
    setDdOpen(false);
    setFromHistory(false);
    requestAnimationFrame(() => ddTriggerRef.current?.focus());
  }, []);

  const goHistory = useCallback(() => {
    setStep("history");
    setFromHistory(false);
  }, []);

  const openHistoryEntry = useCallback((o: SendOutcome) => {
    setOutcome(o);
    setFromHistory(true);
    setStep("done");
  }, []);

  /** Retry: open a chat with the same agent, the original context pre-filled
   *  in the composer — one ⌘↵ re-sends it. */
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

  /** Send to another agent: pre-fill the composer and open the picker — the
   *  choice is explicit. */
  const resendWithAnotherFromHistory = useCallback((o: SendOutcome) => {
    setDraft(captureSnippet(o.capture ?? null) ?? "");
    setDraftSignal((n) => n + 1);
    goHome();
    // The picker opens with the context ready — draw attention to it.
    setDdOpen(true);
    setDdOpenTick((n) => n + 1);
  }, [goHome]);

  const clearAllHistory = useCallback(async () => {
    try {
      await clearHistory();
      setHistory([]);
    } catch (e) {
      setErrorText(String(e));
      setStep("error");
    }
  }, []);

  // Clamp the dropdown highlight when the list shrinks (payload refresh).
  useEffect(() => {
    const len = ddItems.length;
    if (len > 0 && ddHighlight >= len) setDdHighlight(0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [payload, ddItems.length]);

  /** Reopening the dropdown always starts from a sane highlight: the agent
   *  being chatted with, else the first entry. */
  useEffect(() => {
    if (!ddOpen) return;
    const cur = chatAgent ? ddItems.findIndex((it) => it.agent.meta.id === chatAgent.id) : -1;
    setDdHighlight(cur >= 0 ? cur : 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ddOpen, ddOpenTick]);

  const onKeyDown = useCallback(
    (e: KeyboardEvent) => {
      // Overlays own the keyboard while open. The welcome answers one
      // question (start now?); the cheat sheet just needs Escape/close.
      if (showWelcome) {
        if (e.key === "Escape") hide();
        else if (e.key === "Enter") dismissWelcome();
        return;
      }
      if (showShortcuts) {
        if (e.key === "Escape") setShowShortcuts(false);
        return;
      }
      // `?` opens the cheat sheet — but only when the user isn't typing
      // (in the search box or an editor, `?` belongs to the text).
      if (e.key === "?") {
        const t = e.target as HTMLElement | null;
        const typing = t?.tagName === "INPUT" || t?.tagName === "TEXTAREA";
        if (!typing) {
          e.preventDefault();
          setShowShortcuts(true);
          return;
        }
      }
      if (e.key === "Escape") {
        if (step === "home" && ddOpen) {
          // First Escape closes only the picker; the sheet stays up.
          setDdOpen(false);
        } else if (step === "history") {
          hide();
        } else if (step === "done" && fromHistory) {
          setStep("history");
        } else if (step === "chat") {
          goHome();
        } else {
          // home / done / error — dismiss. A handoff in flight keeps running;
          // the completion notification still fires.
          hide();
        }
        return;
      }
      // Typing in the chat composer must not trigger palette shortcuts.
      if ((e.target as HTMLElement | null)?.tagName === "TEXTAREA") return;

      if (step === "home") {
        const list = ddItems;
        if (!ddOpen) {
          // ArrowDown opens the picker; Enter/ArrowUp open it too — the
          // palette is keyboard-first, the menu is always one press away.
          if (e.key === "ArrowDown" || e.key === "Enter" || e.key === "ArrowUp") {
            e.preventDefault();
            setDdOpen(true);
            setDdOpenTick((n) => n + 1);
          }
          return;
        }
        if (list.length === 0) return;
        if (e.key === "ArrowDown") {
          e.preventDefault();
          setDdHighlight((s) => (s + 1) % list.length);
        } else if (e.key === "ArrowUp") {
          e.preventDefault();
          setDdHighlight((s) => (s - 1 + list.length) % list.length);
        } else if (e.key === "Enter") {
          e.preventDefault();
          chooseFromDropdown(list[ddHighlight]);
        }
      }
    },
    [
      step,
      ddItems,
      ddOpen,
      ddHighlight,
      chooseFromDropdown,
      goHome,
      hide,
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
  }, [outcome, copyText_]);// -------------------------------------------------------------------------
// Render
// -------------------------------------------------------------------------

  /**
   * The "Show activity" payload — real measurements only. The phase is
   * derived from the output stream (never guessed): no output yet →
   * "Sending context", output flowing → "Agent responding", output idle past
   * {@link WRAPPING_IDLE_MS} → "Wrapping up" (and straight back if output
   * resumes). Output volume is the live stream while sending, and the
   * receipt's stdout+stderr once the handoff completed.
   */
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
      : [
          outcome?.receipt?.stdout ?? "",
          outcome?.receipt?.stderr ?? "",
        ]
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

  return (
    <div
      className={dragActive ? "palette dropping" : "palette"}
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      <div className="palette-stack">
      <div className="palette-card">
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
              // One-tap add from the welcome card; reflect it instantly so
              // the row flips to "Added" without waiting for the daemon.
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
        {/* The palette is a borderless window — this strip is the only drag
            handle. `data-tauri-drag-region` on the header and its (non-
            interactive) children makes it draggable; see palette.css. */}
        <header className="palette-header" onDoubleClick={hide} data-tauri-drag-region>
          <span className="logo" data-tauri-drag-region>
            <HandoverMark size={16} />
          </span>
          <span className="title" data-tauri-drag-region>
            Handover
          </span>
          <span className="hotkey-hint" data-tauri-drag-region>
            <Icon name="keyboard" size={12} />
            {formatShortcut(payload?.shortcut ?? "CmdOrCtrl+Shift+A")}
          </span>
          {/* Tools live in the header (formerly the floating dock below). */}
          <div className="header-tools">
            <button
              type="button"
              className={`tool-btn${step === "home" || step === "chat" ? " active" : ""}`}
              title="Agents — chat"
              aria-label="Agents — chat"
              onClick={goHome}
            >
              <Icon name="chat" size={15} />
            </button>
            <button
              type="button"
              className={`tool-btn${step === "history" || step === "done" ? " active" : ""}`}
              title="Recent handoffs"
              aria-label="Recent handoffs"
              onClick={goHistory}
            >
              <Icon name="history" size={15} />
            </button>
            <span className="tool-sep" aria-hidden="true" />
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
              title="Settings"
              aria-label="Open settings"
              onClick={openSettings}
            >
              <Icon name="settings" size={15} />
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

          {/* The picker + its chat are ONE surface ("home"); legacy "chat"
              steps (retry / keep-chatting / sends) land on the same thing. */}
          {(step === "home" || step === "chat") && payload && (
            <div key={`home-${generationRef.current}`} className="step-in" style={{ display: "contents" }}>
              {draft && (
                <div className="captured-line" data-testid="captured-line">
                  <Icon name="clipboard" size={12} />
                  <span className="captured-text">Will start the chat with: {truncate(draft, 80)}</span>
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
              {/* The picker + its chat share one surface — choosing an agent
                  swaps the thread beneath the trigger, it never navigates. */}
              <AgentDropdown
                groups={ddGroups}
                selectedAgentId={chatAgent?.id ?? null}
                open={ddOpen}
                highlight={ddHighlight}
                onHighlight={setDdHighlight}
                onToggle={() => {
                  setDdOpen((v) => !v);
                  setDdOpenTick((n) => n + 1);
                }}
                onClose={() => setDdOpen(false)}
                onChoose={chooseFromDropdown}
                onOpenSettings={openSettings}
                triggerRef={ddTriggerRef}
              />
              {ddGroups.length === 0 && (
                <div className="empty-state" data-testid="empty-state">
                  <span className="status-icon warn" aria-hidden="true">
                    <Icon name="agentLocal" size={20} />
                  </span>
                  <h2>No agents are available yet.</h2>
                  <p>Running agents (Hermes, Codex, Claude Code, …) appear here automatically.</p>
                  <div className="status-actions">
                    <button type="button" className="btn primary" onClick={openSettings}>
                      <Icon name="settings" size={13} />
                      Open Settings to add an agent
                    </button>
                  </div>
                </div>
              )}
              {chatAgent && (
                <ChatView
                  agent={chatAgent}
                  sessionId={chatSessionId}
                  session={chatLiveSession}
                  turns={chatTurns}
                  sendingText={chatPendingText}
                  liveText={live}
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
            </div>
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
              <div className="agents-head">
                <span className="agents-title">Recent handoffs · this session</span>
              </div>
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
            {step === "history"
              ? "↵ open · esc close"
              : step === "chat"
                ? "↵ send · ⇧↵ newline"
                : step === "home"
                  ? ddOpen
                    ? "↑↓ choose an agent · ↵ chat · esc close"
                    : "↓ open agents · esc close"
                  : "esc close"}
          </span>
        </footer>
      </div>
      </div>
    </div>
  );
}
