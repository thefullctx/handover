import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { formatShortcut } from "../lib/format";
import {
  setAppearance,
  setLaunchAtStartup,
  setNotifications,
  setShortcut,
  setUiOpacity,
} from "../lib/tauri";
import type { AppInfo } from "../lib/types";

const APPEARANCES = ["system", "light", "dark"] as const;

interface Props {
  info: AppInfo;
  onInfoChange: (patch: Partial<AppInfo>) => void;
  onError: (msg: string) => void;
}

/**
 * General preferences — every control here works:
 * appearance, the configurable global shortcut, launch at startup,
 * notifications, and quick send.
 */
const OPACITY_MIN = 40; // percent — matches backend clamp 0.4
const OPACITY_MAX = 100;

export default function GeneralSection({ info, onInfoChange, onError }: Props) {
  const [recording, setRecording] = useState(false);
  const [recordError, setRecordError] = useState("");
  const [savingShortcut, setSavingShortcut] = useState(false);
  const recorderRef = useRef<HTMLSpanElement>(null);
  const opacitySaveTimer = useRef<number | null>(null);
  /** Local percent while dragging so the slider feels instant. */
  const [opacityPct, setOpacityPct] = useState(() =>
    Math.round((info.ui_opacity ?? 1) * 100)
  );

  useEffect(() => {
    setOpacityPct(Math.round((info.ui_opacity ?? 1) * 100));
  }, [info.ui_opacity]);

  // Focus the recorder the moment it appears so typing the new combo works
  // immediately — no extra click.
  useEffect(() => {
    if (recording) recorderRef.current?.focus();
  }, [recording]);

  const setAppearance_ = useCallback(
    async (a: (typeof APPEARANCES)[number]) => {
      try {
        await setAppearance(a);
        onInfoChange({ appearance: a });
      } catch (e) {
        onError(String(e));
      }
    },
    [onInfoChange, onError]
  );

  /** Live CSS update + debounced persist so palette and Settings stay in sync. */
  const onOpacityInput = useCallback(
    (pct: number) => {
      const clamped = Math.min(OPACITY_MAX, Math.max(OPACITY_MIN, pct));
      setOpacityPct(clamped);
      const opacity = clamped / 100;
      // Immediate visual feedback in this window (other windows get the event).
      document.documentElement.style.setProperty("--ui-opacity", String(opacity));
      onInfoChange({ ui_opacity: opacity });
      if (opacitySaveTimer.current !== null) {
        window.clearTimeout(opacitySaveTimer.current);
      }
      opacitySaveTimer.current = window.setTimeout(() => {
        void setUiOpacity(opacity)
          .then((saved) => onInfoChange({ ui_opacity: saved }))
          .catch((e) => onError(String(e)));
      }, 120);
    },
    [onInfoChange, onError]
  );

  useEffect(() => {
    return () => {
      if (opacitySaveTimer.current !== null) {
        window.clearTimeout(opacitySaveTimer.current);
      }
    };
  }, []);

  const toggle = useCallback(
    async (fn: (v: boolean) => Promise<void>, value: boolean, patch: Partial<AppInfo>) => {
      try {
        await fn(value);
        onInfoChange(patch);
      } catch (e) {
        onError(String(e));
      }
    },
    [onInfoChange, onError]
  );

  /** Arrow-key roving selection, matching native macOS segmented controls. */
  const onAppearanceKey = useCallback(
    (e: ReactKeyboardEvent<HTMLDivElement>) => {
      const idx = APPEARANCES.indexOf(info.appearance as (typeof APPEARANCES)[number]);
      let next: number;
      if (e.key === "ArrowRight") next = (idx + 1) % APPEARANCES.length;
      else if (e.key === "ArrowLeft") next = (idx - 1 + APPEARANCES.length) % APPEARANCES.length;
      else if (e.key === "Home") next = 0;
      else if (e.key === "End") next = APPEARANCES.length - 1;
      else return;
      e.preventDefault();
      void setAppearance_(APPEARANCES[next]);
    },
    [info.appearance, setAppearance_]
  );

  /** Builds a `CmdOrCtrl+Shift+K`-style accelerator from a keydown event. */
  const acceleratorFromEvent = useCallback((e: ReactKeyboardEvent): string => {
    const parts: string[] = [];
    const isMac = navigator.platform.toLowerCase().includes("mac");
    if (e.metaKey && isMac) parts.push("Cmd");
    else if (e.ctrlKey && !isMac) parts.push("Ctrl");
    if (e.altKey) parts.push("Alt");
    if (e.shiftKey) parts.push("Shift");

    const key = e.key;
    let code: string;
    if (/^[a-zA-Z0-9]$/.test(key)) code = key.toUpperCase();
    else if (key.startsWith("F") && /^F([1-9]|1[0-9]|2[0-4])$/.test(key)) code = key;
    else if (["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(key))
      code = key.replace("Arrow", "");
    else if (key === " ") code = "Space";
    else if (key === "Tab") code = "Tab";
    else if (key === "Enter") code = "Enter";
    else if (key === "Escape") code = "Esc";
    else if (key === "Backspace") code = "Backspace";
    else if (key === "Delete") code = "Delete";
    else return ""; // modifier-only or unsupported key

    const joined = [...parts, code].join("+");
    // Portable form: "Cmd" → "CmdOrCtrl" on macOS so the same config works
    // on Linux (where the modifier becomes Ctrl).
    return joined.replace(/^Cmd\+/, "CmdOrCtrl+");
  }, []);

  const beginRecord = useCallback(() => {
    setRecordError("");
    setRecording(true);
  }, []);

  const onRecordKey = useCallback(
    async (e: ReactKeyboardEvent) => {
      if (!recording) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" && !(e.metaKey || e.ctrlKey || e.altKey || e.shiftKey)) {
        setRecording(false);
        return;
      }
      const accel = acceleratorFromEvent(e);
      if (!accel) {
        setRecordError("Include a key, e.g. ⌘⇧K.");
        return;
      }
      setRecording(false);
      setSavingShortcut(true);
      try {
        await setShortcut(accel);
        onInfoChange({ shortcut: accel });
      } catch (err) {
        setRecordError(String(err));
      } finally {
        setSavingShortcut(false);
      }
    },
    [recording, acceleratorFromEvent, onInfoChange]
  );

  return (
    <>
      <div className="settings-section">Appearance</div>
      <div className="info-row">
        <div>
          <div className="info-label">Appearance</div>
          <div className="info-desc">Follow the system, or force light or dark.</div>
        </div>
        <div
          className="segmented"
          role="radiogroup"
          aria-label="Appearance"
          onKeyDown={onAppearanceKey}
        >
          {APPEARANCES.map((a) => (
            <button
              key={a}
              type="button"
              role="radio"
              aria-checked={info.appearance === a}
              className={`seg-btn${info.appearance === a ? " active" : ""}`}
              onClick={() => void setAppearance_(a)}
            >
              {a === "system" ? "System" : a === "light" ? "Light" : "Dark"}
            </button>
          ))}
        </div>
      </div>

      <div className="info-row col">
        <div className="opacity-row-head">
          <div>
            <div className="info-label">Window opacity</div>
            <div className="info-desc">
              How solid the palette and Settings look. Applies to both windows.
            </div>
          </div>
          <span className="opacity-value" aria-live="polite">
            {opacityPct}%
          </span>
        </div>
        <input
          type="range"
          className="opacity-slider"
          min={OPACITY_MIN}
          max={OPACITY_MAX}
          step={1}
          value={opacityPct}
          aria-label="Window opacity"
          aria-valuemin={OPACITY_MIN}
          aria-valuemax={OPACITY_MAX}
          aria-valuenow={opacityPct}
          onChange={(e) => onOpacityInput(Number(e.target.value))}
        />
      </div>

      <div className="settings-section">Behavior</div>
      <div className="info-row">
        <div>
          <div className="info-label">Global shortcut</div>
          <div className="info-desc">Open the palette from anywhere.</div>
        </div>
        <div className="shortcut-row">
          {recording ? (
            <span
              ref={recorderRef}
              className="shortcut-record"
              data-testid="shortcut-record"
              onKeyDown={(e) => void onRecordKey(e)}
              tabIndex={0}
              role="button"
              aria-label="Press the new shortcut"
            >
              Press the new shortcut… <kbd>esc</kbd> to cancel
            </span>
          ) : (
            <span className="shortcut-display" data-testid="shortcut-display">
              {formatShortcut(info.shortcut)}
            </span>
          )}
          {!recording && (
            <button
              type="button"
              className="btn ghost"
              onClick={beginRecord}
              disabled={savingShortcut}
            >
              {savingShortcut ? "Saving…" : "Change…"}
            </button>
          )}
        </div>
      </div>
      {recordError && <p className="manage-error">{recordError}</p>}

      <div className="info-row">
        <div>
          <div className="info-label">Launch at startup</div>
          <div className="info-desc">Start Handover quietly when you log in.</div>
        </div>
        <input
          type="checkbox"
          className="switch"
          role="switch"
          aria-label="Launch at startup"
          checked={info.launch_at_startup}
          onChange={(e) =>
            void toggle(setLaunchAtStartup, e.target.checked, { launch_at_startup: e.target.checked })
          }
        />
      </div>

      <div className="info-row">
        <div>
          <div className="info-label">Notifications</div>
          <div className="info-desc">Banners when a handoff is sent or completes.</div>
        </div>
        <input
          type="checkbox"
          className="switch"
          role="switch"
          aria-label="Notifications"
          checked={info.notifications}
          onChange={(e) =>
            void toggle(setNotifications, e.target.checked, { notifications: e.target.checked })
          }
        />
      </div>

      <div className="settings-section">Keyboard shortcuts</div>
      <div className="shortcut-grid" aria-label="Keyboard shortcuts">
        <ShortcutRow keys={[formatShortcut(info.shortcut)]} action="Open the palette from anywhere" />
        <ShortcutRow keys={["↑", "↓"]} action="Choose an agent to chat with" />
        <ShortcutRow keys={["↵"]} action="Chat with the selected agent" />
        <ShortcutRow keys={["⌘", "↵"]} action="Send a chat message" />
        <ShortcutRow keys={["⌘", ","]} action="Open Settings" />
        <ShortcutRow keys={["esc"]} action="Dismiss — a running handoff keeps going" />
      </div>
    </>
  );
}

function ShortcutRow({ keys, action }: { keys: string[]; action: string }) {
  return (
    <div className="shortcut-grid-row">
      <span className="shortcut-grid-keys">
        {keys.map((k, i) => (
          <kbd key={i}>{k}</kbd>
        ))}
      </span>
      <span className="shortcut-grid-action">{action}</span>
    </div>
  );
}
