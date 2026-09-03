import { describe, expect, it } from "vitest";
import { cleanReply, filterLiveStream } from "../lib/format";

describe("cleanReply — CLI/TUI chrome stripped to plain chat text", () => {
  it("reduces Hermes' full TUI dump to just the answer", () => {
    const raw = `Query: hi
Initializing agent...
↻ Resumed session 20260814_033539_0756df "Add to persistent memory" (4 user
messages, 8 total messages)
⚡ YOLO mode restored from session — all commands auto-approved. /yolo to turn
off.
────────────────────────────────────────


┌─ Reasoning ──────────────────────────────────────────────────────────────────┐

The user said "hi".
This is a greeting. I should respond politely and ask how I can help them.
└──────────────────────────────────────────────────────────────────────────────┘

╭─ C:\\> HERMES ────────────────────────────────────────────────────────────────╮
Hello! How can I help you today?
╰──────────────────────────────────────────────────────────────────────────────╯

Resume this session with:
  hermes --resume 20260814_033539_0756df
  hermes -c "Add to persistent memory"

Session:        20260814_033539_0756df
Title:          Add to persistent memory
Duration:       14s
Messages:       10 (5 user, 0 tool calls)`;

    expect(cleanReply(raw)).toBe("Hello! How can I help you today?");
  });

  it("drops the session-info footer that quiet mode still prints", () => {
    const raw = `Hello! How can I help you today?

Resume this session with:
  hermes --resume 20260814_033539_0756df
  hermes -c "Add to persistent memory"

Session:        20260814_033539_0756df
Title:          Add to persistent memory
Duration:       14s
Messages:       10 (5 user, 0 tool calls)`;

    expect(cleanReply(raw)).toBe("Hello! How can I help you today?");
  });

  it("passes a plain multi-line reply through unchanged", () => {
    const reply = "The error is a null deref on line 12.\n\nAdd a guard:\n\n```\nif (x == null) return;\n```";
    expect(cleanReply(reply)).toBe(reply);
  });

  it("trims leading announce lines but keeps the first real line", () => {
    const raw = "\n\nQuery: fix this\nInitializing agent...\nThe fix is a null check.\n";
    expect(cleanReply(raw)).toBe("The fix is a null check.");
  });

  it("keeps the reply when the reasoning box has no closing border (quiet mode)", () => {
    // Hermes -Q prints the reasoning header but NO closing └ border — the
    // old skip-until-└ logic ate everything after the header.
    const raw = `\n┌─ Reasoning ──────────────────────────────────────────────────────────────────┐\n\n1. Mercury\n2. Venus\n3. Earth\n`;
    expect(cleanReply(raw)).toBe("1. Mercury\n2. Venus\n3. Earth");
  });

  it("never swallows the answer when reasoning content follows an unclosed box", () => {
    // Real quiet-mode capture: reasoning content, then the answer as the
    // final line — no closing border anywhere. The reply must survive.
    const raw = `\n┌─ Reasoning ──────────────────────────────────────────────────────────────────┐\n\nThe user said "hi".\nI should respond politely and ask how I can assist them.\nThe user said "hi".\nHello! How can I help you today?\n`;
    expect(cleanReply(raw)).toBe(
      "The user said \"hi\".\nI should respond politely and ask how I can assist them.\nThe user said \"hi\".\nHello! How can I help you today?"
    );
  });

  it("drops the restored-workspace notice hermes prints on resume", () => {
    // Real capture (resume with -Q --reasoning none): the workspace notice
    // is the first line of stdout on every resume.
    const raw = `↪ restored workspace dir: /Users/user/Projects/handover\nHello! How can I help you today?\n`;
    expect(cleanReply(raw)).toBe("Hello! How can I help you today?");
  });

  it("cleans workspace notice + empty reasoning box + multi-line answer", () => {
    // Real capture: workspace notice, then an EMPTY reasoning box header
    // (quiet mode prints it without a closing border), then the answer.
    const raw = `↪ restored workspace dir: /Users/user/Projects/handover\n\r\n┌─ Reasoning ──────────────────────────────────────────────────────────────────┐\r\n\r\n\r\nWe are currently in a new session. Our most recent previous sessions were:\n\n* \`@session:default/20260814_165704_47d106\` (Title: "Repeat TST #4")\n* \`@session:default/20260814_165546_777925\` (Title: "Repeat TST #2")\n* \`@session:default/20260814_165633_8c90b7\` (Title: "List three planets")\n`;
    expect(cleanReply(raw)).toBe(
      "We are currently in a new session. Our most recent previous sessions were:\n\n* `@session:default/20260814_165704_47d106` (Title: \"Repeat TST #4\")\n* `@session:default/20260814_165546_777925` (Title: \"Repeat TST #2\")\n* `@session:default/20260814_165633_8c90b7` (Title: \"List three planets\")"
    );
  });

  it("returns empty for empty input", () => {
    expect(cleanReply("")).toBe("");
    expect(cleanReply("   \n\n  ")).toBe("");
  });
});

describe("filterLiveStream — banner noise never reaches the streaming bubble", () => {
  it("strips the exact resume banner deca saw (stdout + stderr lines)", () => {
    const raw = `↻ Resumed session 20260823_134105_802edf "Friendly greeting #8" (21 user messages, 352 total messages)\nModel restored from session: x-preview-f-free (opencode-free)`;
    expect(filterLiveStream(raw)).toBe("");
  });

  it("keeps genuine reply text and drops banners around it", () => {
    const raw = `↪ restored workspace dir: /Users/user\n↻ Resumed session abc "t" (1 user messages, 2 total messages)\nModel restored from session: mtplx-qwen38-27b-optimized-quality\nWorking on it — found the bug.\nFix incoming.`;
    expect(filterLiveStream(raw)).toBe("Working on it — found the bug.\nFix incoming.");
  });

  it("never eats a reply that merely mentions the words", () => {
    const raw = "The session resumed session state successfully.";
    expect(filterLiveStream(raw)).toBe(raw);
  });

  it("passes plain streaming text through untouched", () => {
    expect(filterLiveStream("Thinking…\nLine two")).toBe("Thinking…\nLine two");
  });
});
