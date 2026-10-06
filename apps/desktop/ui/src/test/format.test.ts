import { describe, expect, it } from "vitest";
import { cleanReply } from "../lib/format";

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
    const raw = `↪ restored workspace dir: /Users/user/Projects/handover\n\r\n┌─ Reasoning ──────────────────────────────────────────────────────────────────┐\r\n\r\n\r\nWe are currently in a new session. Our most recent previous sessions were:\n\n* \`@session:default/20260814_165704_47d106\` (Title: "Follow-up check")\n* \`@session:default/20260814_165546_777925\` (Title: "Follow-up check")\n* \`@session:default/20260814_165633_8c90b7\` (Title: "Docs pass")\n`;
    expect(cleanReply(raw)).toBe(
      "We are currently in a new session. Our most recent previous sessions were:\n\n* `@session:default/20260814_165704_47d106` (Title: \"Follow-up check\")\n* `@session:default/20260814_165546_777925` (Title: \"Follow-up check\")\n* `@session:default/20260814_165633_8c90b7` (Title: \"Docs pass\")"
    );
  });

  it("returns empty for empty input", () => {
    expect(cleanReply("")).toBe("");
    expect(cleanReply("   \n\n  ")).toBe("");
  });
});
