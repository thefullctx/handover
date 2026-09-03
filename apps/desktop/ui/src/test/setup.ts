import "@testing-library/jest-dom/vitest";

// jsdom doesn't implement requestAnimationFrame unless pretendToBeVisual is
// set. Handover's palette awaits rAF before IPC round-trips and uses it to
// focus the search field — shim it so tests run headless.
if (typeof globalThis.requestAnimationFrame !== "function") {
  globalThis.requestAnimationFrame = (cb: FrameRequestCallback) =>
    window.setTimeout(() => cb(Date.now()), 0);
  globalThis.cancelAnimationFrame = (id: number) => window.clearTimeout(id);
}
