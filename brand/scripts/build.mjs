// Builds the Handover brand kit: every SVG is generated from the one glyph
// geometry below, then rendered to PNG with headless Chromium.
//
//   cd brand/scripts && npm install && node build.mjs
//
// Needs Chromium: set CHROME to a chrome / chrome-headless-shell binary, or
// have Playwright's headless shell cached (npx playwright install chromium).
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import opentype from "opentype.js";

const BRAND = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO = path.resolve(BRAND, "..");
const out = (p) => {
  const f = path.join(BRAND, p);
  fs.mkdirSync(path.dirname(f), { recursive: true });
  return f;
};

// Monochrome: ink on white, and the reverse. No accent colour; emphasis comes
// from weight and contrast.
export const COLORS = {
  ink: "#111111",
  inkSoft: "#3a3a3a",
  muted: "#6f6f6f",
  faint: "#9a9a9a",
  line: "#e3e3df",
  lineStrong: "#d3d3ce",
  soft: "#f6f6f4",
  white: "#ffffff",
  night: "#0e0e0f",
  nightRaised: "#1a1a1b",
  snow: "#f5f5f5",
};
const { ink, white, snow, line } = COLORS;

// --- The glyph -------------------------------------------------------------
// Designed on a 120-unit tile: two stems (the H) with the ball mid-pass where
// the crossbar would be. Bounding box is x 30..90, y 26..94, centred on 60,60.
const GLYPH = { x0: 30, y0: 26, w: 60, h: 68 };
function glyph({ fill }) {
  return [
    `<rect x="30" y="26" width="16" height="68" rx="8" fill="${fill}"/>`,
    `<rect x="74" y="26" width="16" height="68" rx="8" fill="${fill}"/>`,
    `<circle cx="60" cy="60" r="9" fill="${fill}"/>`,
  ].join("");
}
const ON_DARK = { fill: snow };
const ON_LIGHT = { fill: ink };
const MONO = { fill: "#000000" };
// White tiles get a hairline so their edge survives on white backgrounds.
const RIM = `stroke="${line}" stroke-width="6"`;

const svg = (w, h, body, vb = `0 0 ${w} ${h}`) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="${vb}">${body}</svg>\n`;
// Glyph placed so its 120-unit tile maps onto (x, y, size).
const placed = (x, y, size, style) =>
  `<g transform="translate(${x} ${y}) scale(${size / 120})">${glyph(style)}</g>`;

// --- Wordmark (Geist SemiBold, outlined so it needs no font) ---------------
const font = opentype.loadSync(path.join(BRAND, "fonts/Geist-SemiBold.ttf"));
const capHeight = font.tables.os2.sCapHeight / font.unitsPerEm;
function wordmark(text, capPx) {
  const size = capPx / capHeight;
  const p = font.getPath(text, 0, 0, size, { kerning: true });
  const bb = p.getBoundingBox();
  return { d: p.toPathData(2), x1: bb.x1, y1: bb.y1, w: bb.x2 - bb.x1, h: bb.y2 - bb.y1, size };
}

function lockup(style, textFill, stacked = false) {
  return padded(lockupTight(style, textFill, stacked), 3);
}
// Grows an SVG's canvas by `pad` on every side so strokes and rounded ends
// never touch the image edge.
function padded(src, pad) {
  const [, w, h] = src.match(/width="([\d.]+)" height="([\d.]+)"/).map(Number);
  const W = w + 2 * pad, H = h + 2 * pad;
  return src
    .replace(/width="[\d.]+" height="[\d.]+" viewBox="[^"]+"/, `width="${W}" height="${H}" viewBox="${-pad} ${-pad} ${W} ${H}"`);
}
function lockupTight(style, textFill, stacked) {
  const gh = 68; // glyph height in px at scale 1 (120-unit tile at 120px)
  const wm = wordmark("Handover", gh * 0.62);
  if (!stacked) {
    const gap = 26;
    const w = GLYPH.w + gap + wm.w;
    const h = gh;
    const tx = GLYPH.w + gap - wm.x1;
    const ty = (h - wm.h) / 2 - wm.y1;
    return svg(Math.ceil(w), h, `<g transform="translate(${-GLYPH.x0} ${-GLYPH.y0})">${glyph(style)}</g><path transform="translate(${tx.toFixed(2)} ${ty.toFixed(2)})" d="${wm.d}" fill="${textFill}"/>`);
  }
  const gap = 22;
  const w = Math.max(GLYPH.w, wm.w);
  const h = gh + gap + wm.h;
  const gx = (w - GLYPH.w) / 2 - GLYPH.x0;
  const tx = (w - wm.w) / 2 - wm.x1;
  const ty = gh + gap - wm.y1;
  return svg(Math.ceil(w), Math.ceil(h), `<g transform="translate(${gx} ${-GLYPH.y0})">${glyph(style)}</g><path transform="translate(${tx.toFixed(2)} ${ty.toFixed(2)})" d="${wm.d}" fill="${textFill}"/>`);
}

// --- SVG sources -------------------------------------------------------------
const files = {
  // macOS app icon grid: 824px tile at 100px inset on a 1024 canvas.
  "logo/handover-app-icon.svg": svg(1024, 1024, `<rect x="103" y="103" width="818" height="818" rx="182" fill="${white}" ${RIM}/>${placed(100, 100, 824, ON_LIGHT)}`),
  // Full-bleed rounded tile: avatars, favicons, in-app header mark.
  "logo/handover-tile.svg": svg(1024, 1024, `<rect x="3" y="3" width="1018" height="1018" rx="228" fill="${white}" ${RIM}/>${placed(0, 0, 1024, ON_LIGHT)}`),
  // Reverse tile for dark surfaces (dark-mode UI, dark social cards).
  "logo/handover-tile-dark.svg": svg(1024, 1024, `<rect width="1024" height="1024" rx="230" fill="${ink}"/>${placed(0, 0, 1024, ON_DARK)}`),
  "logo/handover-glyph-on-dark.svg": svg(GLYPH.w * 4, GLYPH.h * 4, glyph(ON_DARK), `${GLYPH.x0} ${GLYPH.y0} ${GLYPH.w} ${GLYPH.h}`),
  "logo/handover-glyph-on-light.svg": svg(GLYPH.w * 4, GLYPH.h * 4, glyph(ON_LIGHT), `${GLYPH.x0} ${GLYPH.y0} ${GLYPH.w} ${GLYPH.h}`),
  "logo/handover-glyph-mono.svg": svg(GLYPH.w * 4, GLYPH.h * 4, glyph(MONO), `${GLYPH.x0} ${GLYPH.y0} ${GLYPH.w} ${GLYPH.h}`),
  "logo/handover-lockup-on-dark.svg": lockup(ON_DARK, snow),
  "logo/handover-lockup-on-light.svg": lockup(ON_LIGHT, ink),
  "logo/handover-lockup-stacked-on-dark.svg": lockup(ON_DARK, snow, true),
  "logo/handover-lockup-stacked-on-light.svg": lockup(ON_LIGHT, ink, true),
  // Menu-bar template: black + alpha only; macOS tints it.
  "logo/handover-tray-template.svg": svg(44, 44, placed(4, 4, 36, MONO)),
};
for (const [f, s] of Object.entries(files)) fs.writeFileSync(out(f), s);

// --- Rendering ---------------------------------------------------------------
function chromeBin() {
  if (process.env.CHROME) return process.env.CHROME;
  const cache = path.join(os.homedir(), "Library/Caches/ms-playwright");
  const linux = path.join(os.homedir(), ".cache/ms-playwright");
  for (const root of [cache, linux]) {
    if (!fs.existsSync(root)) continue;
    for (const d of fs.readdirSync(root).filter((d) => d.startsWith("chromium_headless_shell")).sort().reverse()) {
      for (const sub of fs.readdirSync(path.join(root, d))) {
        const bin = path.join(root, d, sub, "chrome-headless-shell");
        if (fs.existsSync(bin)) return bin;
      }
    }
  }
  throw new Error("No Chromium found: set CHROME=/path/to/chrome-headless-shell");
}
const CHROME = chromeBin();
const TMP = fs.mkdtempSync(path.join(os.tmpdir(), "handover-brand-"));

function shoot(html, w, h, file) {
  const page = path.join(TMP, `${path.basename(file)}.html`);
  fs.writeFileSync(page, html);
  execFileSync(CHROME, [
    "--headless", "--hide-scrollbars", "--force-device-scale-factor=1",
    "--default-background-color=00000000", `--window-size=${w},${h}`,
    `--screenshot=${out(file)}`, `file://${page}`,
  ], { stdio: "ignore" });
}
const frame = (inner, bg = "transparent") =>
  `<!doctype html><meta charset="utf-8"><style>html,body{margin:0;background:${bg};overflow:hidden}img{display:block}</style>${inner}`;
function png(svgFile, w, h, file) {
  shoot(frame(`<img src="file://${out(svgFile)}" width="${w}" height="${h}">`), w, h, file);
}

for (const s of [1024, 512, 256, 128]) png("logo/handover-app-icon.svg", s, s, `png/handover-app-icon-${s}.png`);
for (const s of [1024, 512, 256, 128]) png("logo/handover-tile.svg", s, s, `png/handover-tile-${s}.png`);
for (const s of [512, 128]) png("logo/handover-tile-dark.svg", s, s, `png/handover-tile-dark-${s}.png`);
png("logo/handover-glyph-on-dark.svg", 240, 272, "png/handover-glyph-on-dark.png");
png("logo/handover-glyph-on-light.svg", 240, 272, "png/handover-glyph-on-light.png");
for (const name of ["lockup-on-dark", "lockup-on-light", "lockup-stacked-on-dark", "lockup-stacked-on-light"]) {
  const src = fs.readFileSync(out(`logo/handover-${name}.svg`), "utf8");
  const [, w, h] = src.match(/width="(\d+)" height="(\d+)"/).map(Number);
  png(`logo/handover-${name}.svg`, w * 4, h * 4, `png/handover-${name}.png`);
}
for (const s of [16, 32, 48, 180, 192, 512]) png("logo/handover-tile.svg", s, s, `favicon/favicon-${s}.png`);
fs.copyFileSync(out("logo/handover-tile.svg"), out("favicon/favicon.svg"));

// --- Social -------------------------------------------------------------------
const fontFace = `@font-face{font-family:Geist;src:url(file://${path.join(BRAND, "fonts/Geist-SemiBold.ttf")});font-weight:600}
@font-face{font-family:Geist;src:url(file://${path.join(BRAND, "fonts/Geist-Medium.ttf")});font-weight:500}
@font-face{font-family:"Geist Mono";src:url(file://${path.join(BRAND, "fonts/GeistMono-Medium.ttf")});font-weight:500}`;
const keycaps = (keys, px) => keys.map((k) =>
  `<span style="display:inline-flex;align-items:center;justify-content:center;min-width:${px}px;height:${px}px;padding:0 ${px * 0.28}px;border-radius:${px * 0.24}px;background:${white};border:1px solid ${COLORS.lineStrong};box-shadow:inset 0 -${px * 0.07}px 0 ${COLORS.line};font:500 ${px * 0.44}px 'Geist Mono';color:${ink}">${k}</span>`
).join(`<span style="color:${COLORS.faint};font:500 ${px * 0.4}px Geist;margin:0 ${px * 0.22}px">→</span>`);
const lockupImg = (h) => `<img src="file://${out("logo/handover-lockup-on-light.svg")}" style="height:${h}px;width:auto">`;
const card = (w, h, body) => frame(`<style>${fontFace}*{box-sizing:border-box}body{font-family:Geist;color:${ink};width:${w}px;height:${h}px;position:relative;background:${white}}</style>${body}`, white);
// The pass: the ball travelling a dotted line, the brand's motion motif.
const passLine = (w, y, x0, x1) => `<div style="position:absolute;left:${x0}px;top:${y}px;width:${x1 - x0}px;border-top:3px dotted ${COLORS.lineStrong}"></div><div style="position:absolute;left:${x1 - 16}px;top:${y - 15}px;width:32px;height:32px;border-radius:50%;background:${ink}"></div>`;
const quiet = (t) => `<span style="color:${COLORS.faint}">${t}</span>`;

shoot(card(400, 400, `<img src="file://${out("logo/handover-glyph-on-light.svg")}" style="position:absolute;left:${200 - 88}px;top:${200 - 100}px;width:176px;height:200px">`), 400, 400, "social/x-avatar.png");

shoot(card(1500, 500, `
  <div style="position:absolute;right:120px;top:120px;text-align:right">
    <div style="font-weight:600;font-size:64px;letter-spacing:-1.6px;line-height:1.08">See something.<br>One hotkey. ${quiet("The right agent.")}</div>
    <div style="margin-top:34px;display:flex;justify-content:flex-end;align-items:center">${keycaps(["⌘⇧A", "↵", "↵"], 54)}</div>
  </div>
  ${passLine(1500, 430, 560, 1380)}`), 1500, 500, "social/x-header.png");

const preview = (w, h) => card(w, h, `
  <div style="position:absolute;left:${w * 0.075}px;top:${h * 0.16}px">${lockupImg(h * 0.105)}</div>
  <div style="position:absolute;left:${w * 0.075}px;top:${h * 0.36}px;font-weight:600;font-size:${h * 0.105}px;letter-spacing:-${h * 0.0025}px;line-height:1.1">See something. One hotkey.<br>${quiet("The right agent.")}</div>
  <div style="position:absolute;left:${w * 0.075}px;bottom:${h * 0.13}px;display:flex;align-items:center;gap:${h * 0.05}px">
    <div style="display:flex;align-items:center">${keycaps(["⌘⇧A", "↵", "↵"], h * 0.075)}</div>
    <div style="font:500 ${h * 0.034}px Geist;color:${COLORS.muted}">Open source · Local-first · macOS &amp; Linux</div>
  </div>
  ${passLine(w, h * 0.13, w * 0.64, w * 0.925)}`);
shoot(preview(1280, 640), 1280, 640, "social/github-social-preview.png");
shoot(preview(1200, 630), 1200, 630, "social/og-image.png");

// Announcement template: edit kicker / title / items, rerun, post.
const post = ({ kicker, title, items }) => card(1600, 900, `
  <div style="position:absolute;left:110px;top:96px">${lockupImg(54)}</div>
  <div style="position:absolute;left:110px;top:210px;font:500 26px 'Geist Mono';color:${COLORS.muted};letter-spacing:1px">${kicker}</div>
  <div style="position:absolute;left:110px;top:256px;font-weight:600;font-size:76px;letter-spacing:-2px;line-height:1.05;width:1300px">${title}</div>
  <div style="position:absolute;left:110px;top:${title.includes("<br>") ? 480 : 400}px;display:grid;gap:22px;width:1380px">${items.map((t) =>
    `<div style="display:flex;gap:22px;align-items:baseline;font:500 34px Geist;color:${COLORS.inkSoft}"><span style="flex:none;width:14px;height:14px;border-radius:50%;background:${ink};transform:translateY(-5px)"></span><span>${t}</span></div>`).join("")}</div>
  <div style="position:absolute;left:110px;bottom:84px;font:500 26px 'Geist Mono';color:${COLORS.muted}">github.com/thefullctx/handover</div>`);
shoot(post({
  kicker: "WHAT'S NEW",
  title: "Calmer palette, sturdier adapters",
  items: [
    "Thinking animation, then the reply. No half-streamed noise",
    "Drift guards tie every agent assumption to a verified version",
    "Hermes adapter verified against v0.21.5",
    "tauri dev works from any folder · a 1-in-10 flaky test fixed",
  ],
}), 1600, 900, "social/post-update-example.png");

// --- Into the app --------------------------------------------------------------
const icons = path.join(REPO, "apps/desktop/src-tauri/icons");
const tray = (s, f) => png("logo/handover-tray-template.svg", s, s, f);
tray(22, "png/tray/trayTemplate.png");
tray(44, "png/tray/trayTemplate@2x.png");
tray(32, "png/tray/trayTemplate-32.png");
tray(1024, "png/tray/trayTemplate-source.png");
for (const f of fs.readdirSync(out("png/tray"))) fs.copyFileSync(out(`png/tray/${f}`), path.join(icons, f));

fs.rmSync(TMP, { recursive: true, force: true });
console.log("brand kit built in", BRAND);
