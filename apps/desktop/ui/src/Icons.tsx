// Handover icon system.
//
// Brand mark is the metal “H” app icon (PNG). Action icons are geometric
// SVG (24×24 viewBox, 1.6 stroke, round joins/caps).

import type { ReactElement } from "react";

interface IconProps {
  size?: number;
  className?: string;
  /** Optional accessibility label (defaults to none — decorative). */
  label?: string;
}

/** Brand mark: metal “H” app icon (bitmap — stays crisp via high-res source). */
export function HandoverMark({ size = 18, className }: IconProps) {
  return (
    <img
      src={new URL("./assets/handover-mark.png", import.meta.url).href}
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      className={className ? `handover-mark ${className}` : "handover-mark"}
      draggable={false}
    />
  );
}

/**
 * Official brand logos for the built-in catalog agents (see
 * assets/agent-logos/README.md for sources). Custom/unknown agents fall back
 * to the neutral local-agent icon so the picker always looks consistent.
 */
const AGENT_LOGO: Record<string, string> = {
  claude: new URL("./assets/agent-logos/claude.png", import.meta.url).href,
  codex: new URL("./assets/agent-logos/codex-color.png", import.meta.url).href,
  hermes: new URL("./assets/agent-logos/hermes.png", import.meta.url).href,
  omp: new URL("./assets/agent-logos/omp-icon.svg", import.meta.url).href,
  droid: new URL("./assets/agent-logos/droid.svg", import.meta.url).href,
};

/** Agents whose logo is drawn light-on-transparent (needs a dark tile in
 *  both themes so the mark stays visible). */
const LOGO_DARK_TILE: ReadonlySet<string> = new Set(["omp"]);

/** An agent's official brand logo, or a neutral fallback icon. */
export function AgentLogo({
  id,
  size = 18,
  className,
}: IconProps & { id: string }) {
  const src = AGENT_LOGO[id];
  if (!src) return <Icon name="agentLocal" size={size} className={className} />;
  const tile = LOGO_DARK_TILE.has(id) ? " agent-logo-tile" : "";
  return (
    <img
      src={src}
      width={size}
      height={size}
      alt=""
      aria-hidden="true"
      className={`agent-logo${tile}${className ? ` ${className}` : ""}`}
      draggable={false}
    />
  );
}

const PATHS: Record<string, ReactElement> = {
  settings: (
    <>
      <circle cx="12" cy="12" r="3.2" />
      <path d="M12 2.6v2.5M12 18.9v2.5M2.6 12h2.5M18.9 12h2.5M5.4 5.4l1.8 1.8M16.8 16.8l1.8 1.8M18.6 5.4l-1.8 1.8M7.2 16.8l-1.8 1.8" />
    </>
  ),
  close: <path d="M6 6l12 12M18 6L6 18" />,
  back: <path d="M14.5 5.5 8 12l6.5 6.5" />,
  chevronRight: <path d="M9.5 5.5 16 12l-6.5 6.5" />,
  chevronDown: <path d="M6 9.5l6 6 6-6" />,
  copy: (
    <>
      <rect x="9" y="9" width="11.5" height="11.5" rx="2.2" />
      <path d="M5 15V5.8A2.8 2.8 0 0 1 7.8 3H15" />
    </>
  ),
  check: <path d="M5 12.6l4.3 4.3L19 7.5" />,
  dots: (
    <>
      <circle cx="5.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="18.5" cy="12" r="1.4" fill="currentColor" stroke="none" />
    </>
  ),
  search: (
    <>
      <circle cx="11" cy="11" r="6.4" />
      <path d="M15.8 15.8 21 21" />
    </>
  ),
  send: <path d="M7 17 17 7M9.2 6.8H17v7.8" />,
  trash: (
    <>
      <path d="M4.2 6.8h15.6" />
      <path d="M9.3 6.8V5.4A1.6 1.6 0 0 1 10.9 3.8h2.2a1.6 1.6 0 0 1 1.6 1.6v1.4" />
      <path d="M6.6 6.8l.9 11.4a2 2 0 0 0 2 1.9h5a2 2 0 0 0 2-1.9l.9-11.4" />
    </>
  ),
  refresh: (
    <>
      <path d="M20 12a8 8 0 1 1-2.4-5.7" />
      <path d="M20.5 3.5v4h-4" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 11v5.2M12 7.6h.01" />
    </>
  ),
  warning: (
    <>
      <path d="M12 3.4 2.6 20.2h18.8L12 3.4Z" />
      <path d="M12 10v4M12 17h.01" />
    </>
  ),
  plus: <path d="M12 5.2v13.6M5.2 12h13.6" />,
  history: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 7.2V12l3.4 2" />
    </>
  ),
  clipboard: (
    <>
      <rect x="5" y="4" width="14" height="17" rx="2.6" />
      <path d="M9 4.2a2 2 0 0 1 2-1.9h2a2 2 0 0 1 2 1.9" />
      <path d="M9 10.2h6M9 14.2h6" />
    </>
  ),
  terminal: (
    <>
      <rect x="3.2" y="4.5" width="17.6" height="15" rx="2.6" />
      <path d="M7.2 9.6l3 2.6-3 2.6M12.6 14.8h4" />
    </>
  ),
  image: (
    <>
      <rect x="3.4" y="5" width="17.2" height="14" rx="2.6" />
      <circle cx="9" cy="10" r="1.7" />
      <path d="M3.4 16l4.4-3.8 3.9 3.2 2.9-2.5 5.9 4.5" />
    </>
  ),
  file: (
    <>
      <path d="M13.6 3.4H7.6a2.4 2.4 0 0 0-2.4 2.4v12.4a2.4 2.4 0 0 0 2.4 2.4h8.8a2.4 2.4 0 0 0 2.4-2.4V8.8l-5.2-5.4Z" />
      <path d="M13.4 3.6V9h5.2" />
    </>
  ),
  link: (
    <>
      <path d="M9.6 14.4l4.8-4.8" />
      <path d="M7.2 16.8 5.5 18.5a3.2 3.2 0 0 1-4.5-4.5L5.8 9.2a3.2 3.2 0 0 1 4.5 0" />
      <path d="M16.8 7.2l1.7-1.7a3.2 3.2 0 0 1 4.5 4.5l-4.8 4.8a3.2 3.2 0 0 1-4.5 0" />
    </>
  ),
  layers: (
    <>
      <path d="M12 3.4 3.4 8.2 12 13l8.6-4.8L12 3.4Z" />
      <path d="M3.4 12.6 12 17.4l8.6-4.8" />
      <path d="M3.4 17 12 21.8 20.6 17" />
    </>
  ),
  agentLocal: (
    <>
      <rect x="3.4" y="4.4" width="17.2" height="15.2" rx="3" />
      <path d="M8 10l2.6 2.3L8 14.6M12.8 14.6h3.2" />
    </>
  ),
  agentCloud: (
    <>
      <path d="M6.8 18.4a4.4 4.4 0 0 1-.5-8.8 6 6 0 0 1 11.6 1.4 3.9 3.9 0 0 1-.9 7.4H6.8Z" />
      <path d="M12 11.6v4M9.9 13.5h4.2" />
    </>
  ),
  bolt: <path d="M13 2.6 5 13.6h6l-1 7.8 8-11h-6l1-7.8Z" />,
  activity: <path d="M2.8 12.4h4.4l2.4-6.6 4.4 12.4 2.4-5.8h4.8" />,
  wrench: (
    <>
      <path d="M14.6 6.4a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.7-3.7a5.8 5.8 0 0 1-7.7 7.7l-6.7 6.7a2 2 0 0 1-2.9-2.9l6.7-6.7a5.8 5.8 0 0 1 7.7-7.7l-3.8 3.6Z" />
    </>
  ),
  doc: (
    <>
      <path d="M13.6 3.4H7.6a2.4 2.4 0 0 0-2.4 2.4v12.4a2.4 2.4 0 0 0 2.4 2.4h8.8a2.4 2.4 0 0 0 2.4-2.4V8.8l-5.2-5.4Z" />
      <path d="M13.4 3.6V9h5.2" />
    </>
  ),
  checklist: (
    <>
      <rect x="5" y="4" width="14" height="16" rx="2.6" />
      <path d="M9 9.4l1.7 1.7 3.8-4M9 14.6l1.7 1.7 3.8-4" />
    </>
  ),
  chat: (
    <>
      <path d="M4.2 5.8A2.4 2.4 0 0 1 6.6 3.4h10.8a2.4 2.4 0 0 1 2.4 2.4v8.4a2.4 2.4 0 0 1-2.4 2.4H9.4l-4.6 4v-4H6.6a2.4 2.4 0 0 1-2.4-2.4V5.8Z" />
    </>
  ),
  keyboard: (
    <>
      <rect x="3" y="6" width="18" height="12" rx="2.2" />
      <path d="M6.8 10h.01M10 10h.01M13.2 10h.01M16.4 10h.01M7 14h10" />
    </>
  ),
  pencil: <path d="M4 20l.9-3.8L15.6 5.5a1.6 1.6 0 0 1 2.3 0l.6.6a1.6 1.6 0 0 1 0 2.3L7.4 19.1 4 20ZM13.5 7.4l3.1 3.1" />,
  eye: (
    <>
      <path d="M2.6 12S6 5.8 12 5.8 21.4 12 21.4 12 18 18.2 12 18.2 2.6 12 2.6 12Z" />
      <circle cx="12" cy="12" r="2.6" />
    </>
  ),
  forward: (
    <>
      <path d="M12 19V5" />
      <path d="M5.5 11.5 12 5l6.5 6.5" />
    </>
  ),
  swap: (
    <>
      <path d="M8 4 4 8l4 4" />
      <path d="M4 8h11a5 5 0 0 1 0 10h-2" />
    </>
  ),
  shield: (
    <>
      <path d="M12 3 4.6 5.8v5.4c0 4.4 3 8 7.4 9.8 4.4-1.8 7.4-5.4 7.4-9.8V5.8L12 3Z" />
      <path d="M8.8 12.2l2.2 2.2 4.4-4.6" />
    </>
  ),
  clock: (
    <>
      <circle cx="12" cy="12" r="8.8" />
      <path d="M12 7.2V12l3.4 2" />
    </>
  ),
  logout: (
    <>
      <path d="M14.5 4H7.4a1.8 1.8 0 0 0-1.8 1.8v12.4a1.8 1.8 0 0 0 1.8 1.8h7.1" />
      <path d="M10 12h10.5M17.5 8.5 21 12l-3.5 3.5" />
    </>
  ),
};

/** A single geometric icon from the Handover set. */
export function Icon({
  name,
  size = 16,
  className,
  label,
}: IconProps & { name: string }) {
  const body = PATHS[name];
  if (!body) return null;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      {body}
    </svg>
  );
}

/** Maps a built-in action to its Handover icon. */
export function actionIcon(id: string): string {
  switch (id) {
    case "fix":
      return "wrench";
    case "investigate":
      return "search";
    case "explain":
      return "doc";
    case "write_tests":
      return "checklist";
    case "ask":
      return "chat";
    default:
      return "bolt";
  }
}
