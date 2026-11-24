import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";
import { setPaletteHeight } from "./tauri";

export const PANEL_CARD_HEIGHT = 500;
const MIN_HOME_HEIGHT = 240;
const MAX_HOME_HEIGHT = 500;
const DURATION_MS = 200;
const EASING = "cubic-bezier(0.22, 1, 0.36, 1)";

/**
 * Keeps the native palette window sized to its content.
 *
 * - `"home"` (the always-unfolded agent list): tracks the natural card height
 *   via ResizeObserver (clamped between 240–500px so long lists scroll cleanly)
 *   and saves it as the default open height.
 * - `"panel"` (chat, history, result, welcome, shortcuts, error): settles at a
 *   consistent 500px panel height so conversations have comfortable breathing room.
 */
export function usePaletteSize(
  paletteRef: RefObject<HTMLElement>,
  cardRef: RefObject<HTMLElement>,
  mode: "home" | "panel"
) {
  const lastCardH = useRef<number | null>(null);
  const homeCardH = useRef<number | null>(null);
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
      if (!canAnimate || Math.abs(from - to) < 2) {
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

    if (mode === "home") {
      card.style.height = "";
      const measured = card.getBoundingClientRect().height;
      const target = Math.min(MAX_HOME_HEIGHT, Math.max(MIN_HOME_HEIGHT, measured || 360));
      homeCardH.current = target;
      const from = lastCardH.current ?? target;
      if (Math.abs(from - target) < 2) {
        settle();
        lastCardH.current = target;
        void setPaletteHeight(Math.round(target + pads), true).catch(() => {});
        return;
      }
      card.style.height = `${from}px`;
      card.style.flex = "0 0 auto";
      animate(from, target, () => {
        settle();
        lastCardH.current = target;
        void setPaletteHeight(Math.round(target + pads), true).catch(() => {});
      });
    } else {
      const target = PANEL_CARD_HEIGHT;
      const from = lastCardH.current ?? homeCardH.current ?? target;
      if (Math.abs(from - target) < 2) {
        settle();
        lastCardH.current = target;
        void setPaletteHeight(Math.round(target + pads), false).catch(() => {});
        return;
      }
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
  }, [mode, paletteRef, cardRef]);

  useEffect(() => {
    const card = cardRef.current;
    const palette = paletteRef.current;
    if (mode !== "home" || !card || !palette || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      if (card.style.height) return;
      const raw = card.getBoundingClientRect().height;
      if (!raw) return;
      const h = Math.min(MAX_HOME_HEIGHT, Math.max(MIN_HOME_HEIGHT, raw));
      if (homeCardH.current !== null && Math.abs(h - homeCardH.current) < 1) return;
      homeCardH.current = h;
      lastCardH.current = h;
      void setPaletteHeight(Math.round(h + paddingOf(palette)), true).catch(() => {});
    });
    ro.observe(card);
    return () => ro.disconnect();
  }, [mode, cardRef, paletteRef]);
}
