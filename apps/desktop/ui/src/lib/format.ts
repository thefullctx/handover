import type {
  Capture,
  ContentKind,
  HandoffPhase,
  LiveSession,
  SendOutcome,
} from "./types";

/** `0:45`-style duration. */
export function fmtDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/** Sub-second-friendly duration for the activity stats: `820 ms` / `2.4 s`.
 *  `fmtDuration` rounds to whole seconds, which reads as `0:00` for a fast
 *  agent — exactly the number the user most wants to compare. */
export function fmtMs(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

/**
 * UTF-8 byte length of a string. NOT `String.length`, which counts UTF-16 code
 * units — the activity stats report bytes so they agree with the `prompt_len`
 * the daemon measures in Rust.
 */
export function byteLength(text: string): number {
  return new TextEncoder().encode(text).length;
}

/** The handoff phase timeline, in order. The activity section renders these
 *  as steps and highlights the one the stream is actually in. */
export const PHASE_STEPS: { id: HandoffPhase; label: string }[] = [
  { id: "sending", label: "Sending context" },
  { id: "responding", label: "Agent responding" },
  { id: "wrapping", label: "Wrapping up" },
];

/** Index of the active phase in [`PHASE_STEPS`]; `-1` once done. */
export function phaseIndex(phase: HandoffPhase): number {
  return PHASE_STEPS.findIndex((s) => s.id === phase);
}

/** Compact byte count for the sending stats: `512 B` / `4.1 KB` / `1.2 MB`. */
export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** First non-empty line of the agent's reply (history rows + notifications). */
export function replySnippet(o: SendOutcome): string {
  const out = cleanReply(o.receipt?.stdout ?? "");
  const line = out.split("\n").find((l) => l.trim());
  if (line) return line.trim();
  return o.error ?? "";
}

/**
 * Strips CLI/TUI chrome from an agent reply so the chat reads like a normal
 * conversation. Removes:
 *  - leading announce lines (Hermes' "Query: …", "Initializing agent...",
 *    "↻ Resumed session …", "⚡ YOLO mode …", dash separators),
 *  - the trailing session-info footer ("Resume this session with:",
 *    "Session:", … — printed even in quiet mode),
 *  - box-drawn "Reasoning" blocks (internal thinking, not the answer) and
 *    any box borders, leaving the reply text itself.
 * Plain-text replies pass through unchanged.
 */
const BOX_CHARS = "─━│┃┌┐└┘├┤┬┴┼╭╮╰╯";
/** Pure border rules (e.g. `───────`, `│`, `└────┘`). */
const BOX_BORDER_RE = new RegExp(`^[${BOX_CHARS}\\s]+$`);
/** Rounded box header/footer with a title (e.g. `╭─ C:\\> HERMES ───╮`). */
const ROUNDED_BOX_RE = /^[╭╰][─━]/;
/** A box header that frames the agent's internal reasoning. */
const REASONING_HEADER_RE = /^[┌╭][─━]*\s*reasoning/i;
/** Announce lines: `Query: hi`, `↻ Resumed session …`, `↪ restored workspace
 *  dir: …`, `⚡ YOLO mode …`. */
const CHROME_LEAD_RE =
  /^(?:[↻↪⚡▶✔✖•>\s-]*)?(query:|initializing agent|resumed session|yolo mode|restored workspace)/i;
/** Trailing session-info footer (`Resume this session with:` / `Session:` …). */
const SESSION_FOOTER_RE =
  /^(resume this session|hermes (--resume|-c )|(session|title|duration|messages):)/i;

export function cleanReply(raw: string): string {
  if (!raw) return "";
  const lines = raw.split("\n");
  // The answer starts at the first line that is neither blank nor chrome. A
  // wrapped continuation of an announce line (hermes wraps at the terminal
  // width) counts as chrome too — but only once we're already inside a
  // chrome run, so a plain lowercase reply is never eaten.
  let start = 0;
  let inChrome = false;
  while (start < lines.length) {
    const l = lines[start].trim();
    if (!l) {
      start += 1;
    } else if (/^[┌╭]/.test(l)) {
      break; // a box header — content follows
    } else if (BOX_BORDER_RE.test(l) || ROUNDED_BOX_RE.test(l) || CHROME_LEAD_RE.test(l)) {
      inChrome = true;
      start += 1;
    } else if (inChrome && /^[a-z).,]/.test(l)) {
      start += 1; // wrapped fragment of the announce block
    } else break;
  }
  // Drop the trailing session-info footer (printed even in quiet mode).
  let end = lines.length;
  for (let i = start; i < lines.length; i += 1) {
    if (SESSION_FOOTER_RE.test(lines[i].trim())) {
      end = i;
      break;
    }
  }
  const body: string[] = [];
  let inReasoning = false;
  for (let i = start; i < end; i += 1) {
    const line = lines[i];
    const t = line.trim();
    if (inReasoning) {
      if (/^[└╰][─━]/.test(t)) inReasoning = false;
      continue;
    }
    if (REASONING_HEADER_RE.test(t)) {
      // A proper reasoning box is closed by a └ border. Hermes' quiet mode
      // prints the header with NO closing border — treat that as an empty
      // box and skip only the header line, never the reply that follows.
      const hasClosing = lines
        .slice(i + 1, end)
        .some((l) => /^[└╰][─━]/.test(l.trim()));
      inReasoning = hasClosing;
      continue;
    }
    if (BOX_BORDER_RE.test(t) || ROUNDED_BOX_RE.test(t)) continue;
    // Strip leftover box-drawing gutters from content lines (e.g. `│text│`).
    body.push(line.replace(new RegExp(`^[${BOX_CHARS}]+|[${BOX_CHARS}]+$`, "g"), "").trimEnd());
  }
  return body.join("\n").trim().replace(/\n{3,}/g, "\n\n");
}

export function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max).trimEnd() + "…";
}

export function kindIcon(kind: ContentKind): string {
  switch (kind) {
    case "text":
      return "doc";
    case "terminal":
      return "terminal";
    case "image":
      return "image";
    case "file":
      return "file";
    case "url":
      return "link";
    case "mixed":
      return "layers";
  }
}

export function kindLabel(kind: ContentKind): string {
  switch (kind) {
    case "text":
      return "Text";
    case "terminal":
      return "Terminal output";
    case "image":
      return "Image";
    case "file":
      return "File";
    case "url":
      return "URL";
    case "mixed":
      return "Multiple";
  }
}

/** The visible capture body (text, path, or joined items). */
export function captureSnippet(capture: Capture | null): string {
  if (!capture) return "";
  const c = capture.content;
  if (c.text && c.text.trim()) return c.text.trim();
  if (c.path) return c.path;
  if (c.items && c.items.length) return c.items.join("\n");
  return "";
}

/** Human status of a source type, e.g. "from Terminal" / "Clipboard". */
export function sourceLabel(capture: Capture | null): string {
  if (!capture) return "";
  const app = capture.source.application;
  if (app) return app;
  switch (capture.source.type) {
    case "clipboard":
      return "Clipboard";
    case "terminal":
      return "Terminal";
    case "manual":
      return "Manual";
    case "file":
      return "File";
    case "image":
      return "Image";
    case "url":
      return "URL";
    default:
      return capture.source.type;
  }
}

/** Compact size/note metadata chips worth showing on the context card. */
export function contextMeta(capture: Capture | null): [string, string][] {
  if (!capture) return [];
  return Object.entries(capture.metadata)
    .filter(([k]) => k !== "capture_method" && k !== "dropped")
    .slice(0, 3);
}

/** Present-tense verb fragment for the sending screen, e.g. "investigating the issue". */
export function actionVerb(actionId: string): string {
  switch (actionId) {
    case "fix":
      return "fixing the issue";
    case "investigate":
      return "investigating the issue";
    case "explain":
      return "explaining the context";
    case "write_tests":
      return "writing tests";
    case "ask":
      return "analyzing the context";
    default:
      return "working on it";
  }
}

/** Outcome-oriented title for the completed result, e.g. "Fix analysis completed". */
export function outcomeTitle(o: SendOutcome): string {
  if (!o.ok) return "Handoff failed";
  switch (o.action_id) {
    case "fix":
      return "Fix analysis completed";
    case "investigate":
      return "Investigation complete";
    case "explain":
      return "Explanation ready";
    case "write_tests":
      return "Tests drafted";
    case "ask":
      return "Recommendation ready";
    default:
      return "Handoff completed";
  }
}

/** User-facing failure line: the daemon's message when present, else the
 *  agent receipt's own detail (e.g. "Exited with status 1") — never a blank
 *  mystery. Non-zero agent exits record a receipt with `error: None`, so
 *  without the receipt fallback every such failure reads "try again" with
 *  the real output hidden. */
export function failureMessage(o: SendOutcome): string {
  return o.error ?? o.receipt?.detail ?? "The handoff failed. Please try again.";
}

/** Availability label for the agent picker: Ready / Not found / Demo. */
export function agentStatusLabel(a: { status: { available: boolean }; meta: { demo?: boolean } }): {
  label: string;
  tone: "ok" | "warn" | "off";
} {
  if (a.meta.demo) return { label: "Demo", tone: "warn" };
  if (a.status.available) return { label: "Ready", tone: "ok" };
  return { label: "Not found", tone: "off" };
}

/** Capability label for an agent, e.g. "Command line agent". */
export function capabilityLabel(kind: string): string {
  switch (kind) {
    case "command":
      return "Command line agent";
    case "openai_compatible":
      return "OpenAI-compatible endpoint";
    default:
      return kind;
  }
}

/** "2:14pm"-style local time for a session chip. */
export function sessionTimeLabel(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** Target-line suffix: "working · live session" vs "last session · 2:14pm".
 *  LIVE means the transcript is actively being written; LAST means it is
 *  quiet but within the staleness cutoff — never "live" for a dead session. */
export function sessionTargetLabel(s: LiveSession): string {
  if (s.activity === "working") return "working · live session";
  return `last session · ${sessionTimeLabel(s.updated_at)}`;
}

/** Compact picker chip: "live · 2:14pm" (working, or the agent's process is
 *  running) / "last · 2:14pm" (quiet, not running). */
export function sessionChipLabel(s: LiveSession, online?: boolean): string {
  const time = sessionTimeLabel(s.updated_at);
  if (online ?? s.activity === "working") return `live · ${time}`;
  return `last · ${time}`;
}

/** Groups outcomes by day label (Today / Yesterday / weekday date). */
export function groupHistoryByDay(
  history: SendOutcome[]
): { label: string; entries: SendOutcome[] }[] {
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const startOfYesterday = new Date(startOfToday);
  startOfYesterday.setDate(startOfToday.getDate() - 1);

  const groups = new Map<string, SendOutcome[]>();
  const labelFor = (d: Date): string => {
    const dayStart = new Date(d.getFullYear(), d.getMonth(), d.getDate());
    if (dayStart.getTime() === startOfToday.getTime()) return "Today";
    if (dayStart.getTime() === startOfYesterday.getTime()) return "Yesterday";
    return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
  };

  for (const o of history) {
    const date = o.created_at ? new Date(o.created_at) : now;
    const label = labelFor(date);
    const list = groups.get(label) ?? [];
    list.push(o);
    groups.set(label, list);
  }
  return [...groups.entries()].map(([label, entries]) => ({ label, entries }));
}

/** Formats an accelerator like `CmdOrCtrl+Shift+A` for display (⌘⇧A). */
export function formatShortcut(accelerator: string): string {
  const parts = accelerator.split("+");
  const glyphs: Record<string, string> = {
    CmdOrCtrl: "⌘",
    Cmd: "⌘",
    Command: "⌘",
    Control: "⌃",
    Ctrl: "⌃",
    Super: "⌘",
    Meta: "⌘",
    Alt: "⌥",
    Option: "⌥",
    Shift: "⇧",
    Enter: "⏎",
    Return: "⏎",
    Space: "Space",
    Tab: "⇥",
    Esc: "⎋",
  };
  return parts
    .map((p) => {
      const up = p.charAt(0).toUpperCase() + p.slice(1);
      if (glyphs[p] || glyphs[up]) return glyphs[p] ?? glyphs[up];
      // Single letters keep a small keycap look.
      return p.length === 1 ? p.toUpperCase() : p;
    })
    .join("");
}
