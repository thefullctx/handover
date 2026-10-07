import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { setPaletteHeight } from "./tauri";

/** Room the agent menu needs below the picker: its max-height (palette.css
 *  `--dd-menu-max`, about four agents plus the group labels and the Settings
 *  link) + the 6px gap under the trigger + a little breathing space. More
 *  agents scroll inside the menu instead of growing the window. */
export const MENU_ROOM = 300 + 6 + 10;
/** Card height before anything has been measured (matches Rust's 210pt
 *  default window minus the palette's padding). */
const FALLBACK_COMPACT_CARD = 154;
const DURATION_MS = 220;
const EASING = "cubic-bezier(0.22, 1, 0.36, 1)";

/**
 * Sizes the palette window to its content.
 *
 * Compact: only the header, the picker and the hint line — the window is
 * exactly as tall as that. Expanded: the same plus room for the open agent
 * menu; the chat and every other panel live at that one height, never taller.
 *
 * The window itself is resized natively (a tall transparent window would
 * still swallow clicks on whatever is behind it). To keep it smooth the card's
 * height is animated: growing resizes the window first and then animates the
 * card down into it; shrinking animates the card first and resizes last.
 */
export function usePaletteSize(
  paletteRef: RefObject<HTMLElement>,
  cardRef: RefObject<HTMLElement>,
  compact: boolean
) {
  const lastCardH = useRef<number | null>(null);
  const compactCardH = useRef<number | null>(null);
  const generation = useRef(0);
  const anim = useRef<Animation | null>(null);

  const paddingOf = (el: HTMLElement) => {
    const cs = getComputedStyle(el);
    return (parseFloat(cs.paddingTop) || 0) + (parseFloat(cs.paddingBottom) || 0);
  };

  useLayoutEffect(() => {
    const palette = paletteRef.current;
    const card = cardRef.current;
    if (!palette || !card) return;
    const gen = ++generation.current;
    anim.current?.cancel();
    const pads = paddingOf(palette);
    const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const canAnimate = typeof card.animate === "function" && !reduce;

    const settle = () => {
      card.style.height = "";
      card.style.flex = "";
    };
    const animate = (from: number, to: number, done: () => void) => {
      if (!canAnimate || from === to) {
        done();
        return;
      }
      const a = card.animate([{ height: `${from}px` }, { height: `${to}px` }], {
        duration: DURATION_MS,
        easing: EASING,
      });
      anim.current = a;
      a.onfinish = () => {
        if (gen === generation.current) done();
      };
    };

    if (compact) {
      // Natural height of the compact content (the .compact class has
      // already stopped the card from stretching).
      card.style.height = "";
      const target = card.getBoundingClientRect().height;
      compactCardH.current = target;
      const from = lastCardH.current ?? target;
      card.style.height = `${from}px`;
      card.style.flex = "0 0 auto";
      animate(from, target, () => {
        settle();
        lastCardH.current = target;
        void setPaletteHeight(Math.round(target + pads), true).catch(() => {});
      });
    } else {
      const base = compactCardH.current ?? FALLBACK_COMPACT_CARD;
      const target = base + MENU_ROOM;
      const from = lastCardH.current ?? base;
      // Hold the card at its current height while the window grows, then
      // let it unfold into the new space.
      card.style.height = `${from}px`;
      card.style.flex = "0 0 auto";
      void setPaletteHeight(Math.round(target + pads), false)
        .catch(() => {})
        .then(() => {
          if (gen !== generation.current) return;
          animate(from, target, () => {
            settle();
            lastCardH.current = target;
          });
        });
    }
  }, [compact, paletteRef, cardRef]);

  // While compact, follow content changes (a drop's "Will start the chat
  // with" line, the empty state) without animating.
  useEffect(() => {
    const card = cardRef.current;
    const palette = paletteRef.current;
    if (!compact || !card || !palette || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (card.style.height) return; // mid-animation
      const h = card.getBoundingClientRect().height;
      if (compactCardH.current !== null && Math.abs(h - compactCardH.current) < 1) return;
      compactCardH.current = h;
      lastCardH.current = h;
      void setPaletteHeight(Math.round(h + paddingOf(palette)), true).catch(() => {});
    });
    ro.observe(card);
    return () => ro.disconnect();
  }, [compact, cardRef, paletteRef]);
}
