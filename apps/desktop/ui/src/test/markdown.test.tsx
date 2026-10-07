import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  copyText: vi.fn(async () => {}),
  openUrl: vi.fn(async () => {}),
}));
vi.mock("../lib/tauri", () => tauri);

import { Markdown, parseBlocks } from "../lib/markdown";

describe("markdown — agent replies render as formatted text", () => {
  it("splits paragraphs, headings, lists, quotes, rules and fenced code", () => {
    const src = [
      "## Fix",
      "First line",
      "second line",
      "",
      "- one",
      "- two",
      "",
      "3. three",
      "4. four",
      "> quoted",
      "---",
      "```ts",
      "const a = 1;",
      "```",
    ].join("\n");
    expect(parseBlocks(src)).toEqual([
      { kind: "h", level: 2, text: "Fix" },
      { kind: "p", text: "First line\nsecond line" },
      { kind: "ul", items: ["one", "two"], start: 1 },
      { kind: "ol", items: ["three", "four"], start: 3 },
      { kind: "quote", text: "quoted" },
      { kind: "hr" },
      { kind: "code", lang: "ts", text: "const a = 1;" },
    ]);
  });

  it("renders inline code, bold and italic without leaving markers behind", () => {
    const { container } = render(<Markdown text={"Use `npm ci`, **not** *npm install*."} />);
    expect(container.querySelector("code")).toHaveTextContent("npm ci");
    expect(container.querySelector("strong")).toHaveTextContent("not");
    expect(container.querySelector("em")).toHaveTextContent("npm install");
    expect(container).toHaveTextContent("Use npm ci, not npm install.");
  });

  it("never turns snake_case words into italics", () => {
    const { container } = render(<Markdown text="call parse_cli_list_output now" />);
    expect(container.querySelector("em")).toBeNull();
    expect(container).toHaveTextContent("call parse_cli_list_output now");
  });

  it("treats HTML in a reply as plain text", () => {
    const { container } = render(<Markdown text={'<img src=x onerror="alert(1)"> <b>hi</b>'} />);
    expect(container.querySelector("img, b")).toBeNull();
    expect(container).toHaveTextContent('<img src=x onerror="alert(1)"> <b>hi</b>');
  });

  it("opens links through openUrl instead of navigating the palette", async () => {
    const user = userEvent.setup();
    render(<Markdown text="See [the docs](https://example.com/docs) or https://example.com/x." />);
    await user.click(screen.getByRole("link", { name: "the docs" }));
    expect(tauri.openUrl).toHaveBeenLastCalledWith("https://example.com/docs");
    // A bare URL stops before trailing punctuation.
    await user.click(screen.getByRole("link", { name: "https://example.com/x" }));
    expect(tauri.openUrl).toHaveBeenLastCalledWith("https://example.com/x");
  });

  it("only links http(s) URLs", () => {
    const { container } = render(<Markdown text="[click](javascript:alert(1)) file:///etc/passwd" />);
    expect(container.querySelector("a")).toBeNull();
  });

  it("copies a code block's text with its Copy button", async () => {
    const user = userEvent.setup();
    render(<Markdown text={"```sh\ncargo test\n```"} />);
    await user.click(screen.getByRole("button", { name: "Copy" }));
    expect(tauri.copyText).toHaveBeenLastCalledWith("cargo test");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
  });
});
