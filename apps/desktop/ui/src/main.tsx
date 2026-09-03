import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import App from "./App";
import SettingsApp from "./Settings";
import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/palette.css";
import "./styles/settings.css";

// The same bundle serves two windows: the borderless command palette ("main")
// and the decorated Settings window ("settings"). Route by window label.
const isSettings = getCurrentWindow().label === "settings";

// macOS-only chrome (Overlay traffic lights, 78px sidebar clearance) is gated
// behind `html.is-mac` so Linux/Windows keep normal padding.
document.documentElement.classList.toggle(
  "is-mac",
  typeof navigator !== "undefined" && /mac/i.test(navigator.platform ?? "")
);

// ---------------------------------------------------------------------------
// Appearance (System / Light / Dark)
// The persisted preference lives in config.toml (`general.appearance`). We
// resolve "system" against prefers-color-scheme and write the *resolved*
// value to <html data-theme="dark|light">, which drives the CSS tokens.
// index.html sets a provisional value first (no flash); we correct it once
// app_info answers and keep it in sync afterwards.
// ---------------------------------------------------------------------------

type Appearance = "system" | "light" | "dark";

const darkMq = window.matchMedia("(prefers-color-scheme: dark)");

function resolvedTheme(a: Appearance): "dark" | "light" {
  return a === "system" ? (darkMq.matches ? "dark" : "light") : a;
}

function applyAppearance(a: Appearance): void {
  document.documentElement.dataset.theme = resolvedTheme(a);
}

// Follow the system when the preference is "system".
let currentAppearance: Appearance = "system";
darkMq.addEventListener("change", () => {
  if (currentAppearance === "system") applyAppearance("system");
});

function applyUiOpacity(opacity: number): void {
  const v = Number.isFinite(opacity) ? Math.min(1, Math.max(0.4, opacity)) : 1;
  document.documentElement.style.setProperty("--ui-opacity", String(v));
}

// Boot: read the persisted preference, then live-update from the backend.
void invoke<{ appearance: string; ui_opacity?: number }>("app_info")
  .then((info) => {
    currentAppearance = (info.appearance ?? "system") as Appearance;
    applyAppearance(currentAppearance);
    if (typeof info.ui_opacity === "number") applyUiOpacity(info.ui_opacity);
  })
  .catch(() => {
    /* keep the provisional system theme */
  });

void listen<Appearance>("appearance:changed", (e) => {
  currentAppearance = e.payload;
  applyAppearance(currentAppearance);
}).catch(() => {
  /* event system unavailable — boot value still applies */
});

void listen<number>("ui-opacity:changed", (e) => {
  applyUiOpacity(e.payload);
}).catch(() => {
  /* event system unavailable */
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isSettings ? <SettingsApp /> : <App />}</React.StrictMode>
);
