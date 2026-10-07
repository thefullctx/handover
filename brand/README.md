# Handover brand kit

<p>
  <img src="png/handover-app-icon-256.png" width="128" alt="Handover app icon">
  &nbsp;
  <img src="png/handover-lockup-on-light.png" height="64" alt="Handover lockup">
</p>

## The mark

**The pass.** An H made of two stems, with the crossbar replaced by a ball
mid-pass: the thing you saw, travelling from you to the agent. The left stem is
solid (you), the right one is quieter (the agent picking it up), and the ball is
always the brightest thing in the mark.

Everything here is generated from one geometry by `scripts/build.mjs`. Change
the mark there, not in the exported files.

| File | Use it for |
|---|---|
| `logo/handover-app-icon.svg` · `png/handover-app-icon-*.png` | The app icon (macOS grid: 824px tile inset 100px on 1024). Source for `tauri icon`. |
| `logo/handover-tile.svg` · `png/handover-tile-*.png` | Full-bleed rounded tile: avatars, favicons, the in-app header mark. |
| `logo/handover-glyph-on-dark.svg` | The glyph alone on ink or any dark surface. |
| `logo/handover-glyph-on-light.svg` | The glyph alone on paper or white. The ball keeps an ink ring, so lime never sits bare on a light background. |
| `logo/handover-glyph-mono.svg` | One-colour uses: stamps, embossing, anything printed in a single ink. |
| `logo/handover-tray-template.svg` · `png/tray/` | macOS menu-bar template (black + alpha; macOS tints it). Copied into `apps/desktop/src-tauri/icons/`. |
| `logo/handover-lockup-*.svg` · `png/handover-lockup-*.png` | Glyph + wordmark, horizontal or stacked, for dark or light backgrounds. |
| `favicon/` | `favicon.svg` plus 16–512px PNGs (180 is the Apple touch icon). |

**Clear space:** keep at least the ball's diameter × 2 empty around the glyph,
and one stem width around a lockup.
**Minimum size:** glyph 16px tall on screen; lockup 20px tall.

Don't:
- recolour the stems or put the ball in any colour but lime (or ink, in mono);
- put bare lime on a light background (use the on-light glyph);
- rotate, outline, add shadows or gradients, or stretch the mark;
- set "Handover" in another typeface next to the mark (use the lockup files).

## Colour

| | Hex | Role |
|---|---|---|
| Ink | `#111214` | Primary background. The app icon tile. |
| Ink raised | `#1b1c20` | Cards, keycaps and panels on ink. |
| Ink line | `#2a2c31` | Borders and dividers on ink. |
| **Lime** | `#d4ff3a` | The accent: the ball, primary buttons, one highlighted phrase. Use sparingly. One lime thing per view reads as intent; five read as noise. |
| Lime deep | `#a8cc1f` | Lime hover/pressed states. |
| Paper | `#f2f2ee` | Text on ink; the light-theme background. |
| Paper dim | `#c9c9c3` | Secondary text on ink. |
| Muted | `#8b8c91` | Captions and tertiary text. |

Lime on ink is 16:1 and paper on ink 17:1 (both WCAG AAA); muted on ink is 5.6:1 (AA). Lime on paper is 1.03:1, effectively invisible. On
light surfaces, use ink text and keep lime for fills (with ink text on top).

## Type

**Geist** (SemiBold 600 for headlines and the wordmark, Medium 500 for body)
and **Geist Mono** (keycaps, code, labels). Both are in `fonts/` under the SIL
Open Font License (`fonts/OFL.txt`), and are also on Google Fonts. Headlines run
tight (letter-spacing about −2.5%); keycaps always use Geist Mono.

## Motion

The brand's one motion idea is **the pass**: the lime ball travels along a
dotted line from left to right and lands. Use it for handoff moments (a send,
a page transition), never as idle decoration.

## Social

| File | Size | Where |
|---|---|---|
| `social/x-avatar.png` | 400×400 | X profile picture (glyph centred for the circular crop). |
| `social/x-header.png` | 1500×500 | X header. Content sits right of centre, clear of the avatar. |
| `social/github-social-preview.png` | 1280×640 | GitHub → Settings → Social preview. Shown when the repo link is shared. |
| `social/og-image.png` | 1200×630 | Website `og:image` / `twitter:image`. |
| `social/post-update-example.png` | 1600×900 | Announcement post. Edit the `post({ … })` call in `scripts/build.mjs` and rebuild. |

## Rebuilding

```bash
cd brand/scripts
npm install
node build.mjs
```

The build needs Chromium to render PNGs. It finds Playwright's cached headless
shell automatically (`npx playwright install chromium-headless-shell`), or set
`CHROME=/path/to/chrome`. It also refreshes the app's tray icons and the in-app
header mark. To regenerate the app icon set after changing the mark:

```bash
cd apps/desktop/src-tauri
../ui/node_modules/.bin/tauri icon ../../../brand/png/handover-app-icon-1024.png
```
