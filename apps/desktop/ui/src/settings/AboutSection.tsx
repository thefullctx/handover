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
 * About — Apple About-panel layout with grouped configuration & collapsible diagnostics.
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
        <HandoverMark size={56} tile />
      </div>
      <p className="about-title">Handover</p>
      <p className="about-version">Version {info.version}</p>
      <p className="about-blurb">
        See something → one hotkey → send it to the right AI agent. A
        lightweight, local-first handoff layer between you and the agents you
        already have.
      </p>

      <div className="settings-group" style={{ width: "100%" }}>
        <div className="g-row">
          <div style={{ minWidth: 0 }}>
            <div className="info-label">Config file</div>
            <div className="info-desc" style={{ wordBreak: "break-all" }}>
              <code>{info.config_path}</code>
            </div>
          </div>
          <button
            type="button"
            className="btn"
            onClick={() => void revealConfig().catch((e) => onError(String(e)))}
          >
            Reveal in Finder
          </button>
        </div>

        <div className="g-row">
          <div>
            <div className="info-label">Diagnostics</div>
            <div className="info-desc">Copy system &amp; agent status for bug reports.</div>
          </div>
          <button type="button" className="btn" onClick={copyDiagnostics}>
            <Icon name={copied ? "check" : "copy"} size={12} />
            {copied ? "Copied" : "Copy diagnostics"}
          </button>
        </div>
      </div>

      <details className="about-diagnostics">
        <summary>View diagnostics report</summary>
        <pre className="diagnostics-box" data-testid="diagnostics">
          {diagnostics}
        </pre>
      </details>

      <p className="about-license">MIT License</p>
    </div>
  );
}
