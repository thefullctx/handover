import { Fragment, useState, type ReactNode } from "react";
import { copyText, openUrl } from "./tauri";

/**
 * A small Markdown renderer for agent replies.
 *
 * Agents (Claude Code, Codex, …) answer in Markdown, so showing the raw text
 * leaves `**`, backticks and fences on screen. This covers what replies
 * actually use — paragraphs, headings, bullet and numbered lists, quotes,
 * rules, fenced code, inline code, bold, italic and links — and leaves
 * anything else as plain text.
 *
 * It builds React elements directly and never sets raw HTML, so nothing in
 * agent output can inject markup or script. Links open through the Rust
 * `open_url` command (http/https only) instead of navigating the palette.
 */

type Block =
  | { kind: "p"; text: string }
  | { kind: "h"; level: number; text: string }
  | { kind: "ul" | "ol"; items: string[]; start: number }
  | { kind: "quote"; text: string }
  | { kind: "code"; lang: string; text: string }
  | { kind: "hr" };

const FENCE = /^\s*(```|~~~)\s*([\w+-]*)\s*$/;
const HEADING = /^(#{1,6})\s+(.*)$/;
const BULLET = /^\s*[-*+]\s+(.*)$/;
const NUMBERED = /^\s*(\d+)[.)]\s+(.*)$/;
const QUOTE = /^\s*>\s?(.*)$/;
const RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;

/** Splits a reply into blocks. Exported for tests. */
export function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let para: string[] = [];
  const flush = () => {
    if (para.length) blocks.push({ kind: "p", text: para.join("\n") });
    para = [];
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const fence = line.match(FENCE);
    if (fence) {
      flush();
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(fence[1])) body.push(lines[i++]);
      blocks.push({ kind: "code", lang: fence[2], text: body.join("\n") });
      continue;
    }
    if (!line.trim()) {
      flush();
      continue;
    }
    if (RULE.test(line)) {
      flush();
      blocks.push({ kind: "hr" });
      continue;
    }
    const h = line.match(HEADING);
    if (h) {
      flush();
      blocks.push({ kind: "h", level: h[1].length, text: h[2] });
      continue;
    }
    const listMatch = line.match(BULLET) ?? line.match(NUMBERED);
    if (listMatch) {
      flush();
      const kind = BULLET.test(line) ? "ul" : "ol";
      const re = kind === "ul" ? BULLET : NUMBERED;
      const start = kind === "ol" ? Number(line.match(NUMBERED)![1]) : 1;
      const items: string[] = [];
      while (i < lines.length && re.test(lines[i])) {
        const m = lines[i].match(re)!;
        items.push(kind === "ul" ? m[1] : m[2]);
        i++;
        // A wrapped continuation line belongs to the item above it.
        while (i < lines.length && /^\s{2,}\S/.test(lines[i]) && !re.test(lines[i]) && !lines[i].match(FENCE)) {
          items[items.length - 1] += "\n" + lines[i].trim();
          i++;
        }
      }
      i--;
      blocks.push({ kind, items, start });
      continue;
    }
    const q = line.match(QUOTE);
    if (q) {
      flush();
      const body = [q[1]];
      while (i + 1 < lines.length && QUOTE.test(lines[i + 1])) body.push(lines[++i].match(QUOTE)![1]);
      blocks.push({ kind: "quote", text: body.join("\n") });
      continue;
    }
    para.push(line);
  }
  flush();
  return blocks;
}

// Inline: `code`, **bold**, *italic* / _italic_, [text](url), bare http(s) URLs.
const INLINE =
  /(`[^`\n]+`)|(\*\*[^*\n]+\*\*|__[^_\n]+__)|(\*[^*\s][^*\n]*\*|\b_[^_\s][^_\n]*_\b)|(\[[^\]\n]+\]\(\s*https?:\/\/[^\s)]+\s*\))|(https?:\/\/[^\s<>()]+[^\s<>().,;:!?'"])/g;

function Link({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a
      className="md-link"
      href={href}
      title={href}
      onClick={(e) => {
        e.preventDefault();
        void openUrl(href);
      }}
    >
      {children}
    </a>
  );
}

/** Renders one run of inline Markdown. Exported for tests. */
export function renderInline(text: string, keyPrefix = "i"): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let n = 0;
  for (const m of text.matchAll(INLINE)) {
    const at = m.index ?? 0;
    if (at > last) out.push(text.slice(last, at));
    const key = `${keyPrefix}-${n++}`;
    const [whole, code, bold, italic, link, bare] = m;
    if (code) out.push(<code key={key}>{code.slice(1, -1)}</code>);
    else if (bold) out.push(<strong key={key}>{renderInline(bold.slice(2, -2), key)}</strong>);
    else if (italic) out.push(<em key={key}>{renderInline(italic.slice(1, -1), key)}</em>);
    else if (link) {
      const label = link.slice(1, link.indexOf("]("));
      const href = link.slice(link.indexOf("](") + 2, -1).trim();
      out.push(<Link key={key} href={href}>{renderInline(label, key)}</Link>);
    } else if (bare) out.push(<Link key={key} href={bare}>{bare}</Link>);
    else out.push(whole);
    last = at + whole.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** A fenced code block with its language and a Copy button. */
function CodeBlock({ lang, text }: { lang: string; text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="md-code">
      <div className="md-code-bar">
        <span>{lang || "code"}</span>
        <button
          type="button"
          className="md-code-copy"
          onClick={() => {
            void copyText(text);
            setCopied(true);
            setTimeout(() => setCopied(false), 1400);
          }}
        >
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre>
        <code>{text}</code>
      </pre>
    </div>
  );
}

/** Renders an agent reply as formatted Markdown. */
export function Markdown({ text }: { text: string }) {
  const blocks = parseBlocks(text);
  return (
    <>
      {blocks.map((b, i) => {
        const k = `b${i}`;
        switch (b.kind) {
          case "p":
            return <p key={k}>{renderInline(b.text, k)}</p>;
          case "h":
            return (
              <p key={k} className={`md-h md-h${Math.min(b.level, 3)}`}>
                {renderInline(b.text, k)}
              </p>
            );
          case "ul":
            return (
              <ul key={k}>
                {b.items.map((it, j) => (
                  <li key={j}>{renderInline(it, `${k}-${j}`)}</li>
                ))}
              </ul>
            );
          case "ol":
            return (
              <ol key={k} start={b.start}>
                {b.items.map((it, j) => (
                  <li key={j}>{renderInline(it, `${k}-${j}`)}</li>
                ))}
              </ol>
            );
          case "quote":
            return <blockquote key={k}>{renderInline(b.text, k)}</blockquote>;
          case "code":
            return <CodeBlock key={k} lang={b.lang} text={b.text} />;
          case "hr":
            return <hr key={k} />;
          default:
            return <Fragment key={k} />;
        }
      })}
    </>
  );
}
