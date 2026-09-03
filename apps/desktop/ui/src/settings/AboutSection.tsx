import { useCallback, useMemo, useState } from "react";
import { HandoverMark, Icon } from "../Icons";
import { copyText, revealConfig } from "../lib/tauri";
import type { AgentConfig, AppInfo } from "../lib/types";

interface Props {
  info: AppInfo;
  agents: AgentConfig[];
  onError: (msg: string) => void;
}

/**
 * About — version, config location with reveal, and a copyable diagnostics
 * blob for support. No roadmap-as-feature here; everything shown is real.
 */
export default function AboutSection({ info, agents, onError }: Props) {
  const [copied, setCopied] = useState(false);

  const diagnostics = useMemo(() => {
    const lines = [
      `Handover ${info.version}`,
      `Platform: ${navigator.platform}`,
      `Config: ${info.config_path}`,
      `Shortcut: ${info.shortcut}`,
      `Appearance: ${info.appearance}`,
      `Notifications: ${info.notifications ? "on" : "off"}`,
      `Launch at startup: ${info.launch_at_startup ? "on" : "off"}`,
      `Agents: ${agents.map((a) => `${a.id}(${a.enabled ? "on" : "off"})`).join(", ") || "none"}`,
    ];
    return lines.join("\n");
  }, [info, agents]);

  const copyDiagnostics = useCallback(() => {
    void copyText(diagnostics)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1600);
      })
      .catch((e) => onError(String(e)));
  }, [diagnostics, onError]);

  return (
    <div className="about-body settings-body">
      <div className="about-logo">
        <HandoverMark size={30} />
      </div>
      <p className="about-title">Handover</p>
      <p className="about-version">Version {info.version}</p>
      <p className="about-blurb">
        See something → one hotkey → send it to the right AI agent. A
        lightweight, local-first handoff layer between you and the agents you
        already have. It is not an AI agent, a chat application, or an
        orchestration platform.
      </p>

      <div className="info-row">
        <span className="info-label">Config file</span>
        <span className="info-value">
          <code>{info.config_path}</code>
        </span>
      </div>

      <div className="settings-actions">
        <button type="button" className="btn" onClick={() => void revealConfig().catch((e) => onError(String(e)))}>
          Reveal config in Finder
        </button>
      </div>

      <div className="settings-section" style={{ width: "100%" }}>
        Diagnostics
      </div>
      <pre className="diagnostics-box" data-testid="diagnostics">
        {diagnostics}
      </pre>
      <div className="settings-actions" style={{ width: "100%" }}>
        <button type="button" className="btn" onClick={copyDiagnostics}>
          <Icon name={copied ? "check" : "copy"} size={13} />
          {copied ? "Copied" : "Copy diagnostics"}
        </button>
      </div>

      <p className="about-license">MIT</p>
    </div>
  );
}
