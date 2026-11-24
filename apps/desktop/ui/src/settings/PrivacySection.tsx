import { useCallback, useEffect, useState } from "react";
import { Icon } from "../Icons";
import { clearHistory, setExcludedPaths } from "../lib/tauri";

interface Props {
  excludedPaths: string[];
  onExcludedPathsChange: (patterns: string[]) => void;
  onError: (msg: string) => void;
}

/**
 * Privacy — grouped inset rows for excluded paths, local-first guarantee,
 * and memory-only history clearing.
 */
export default function PrivacySection({ excludedPaths, onExcludedPathsChange, onError }: Props) {
  const [draft, setDraft] = useState(excludedPaths.join("\n"));
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);

  useEffect(() => {
    if (!dirty) setDraft(excludedPaths.join("\n"));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [excludedPaths]);

  const save = useCallback(async () => {
    const patterns = draft
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);
    setSaving(true);
    try {
      await setExcludedPaths(patterns);
      onExcludedPathsChange(patterns);
      setDirty(false);
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  }, [draft, onExcludedPathsChange, onError]);

  const clear = useCallback(async () => {
    try {
      await clearHistory();
      setConfirmClear(false);
    } catch (e) {
      onError(String(e));
    }
  }, [onError]);

  return (
    <>
      <div className="settings-section">Excluded paths</div>
      <div className="settings-group">
        <div className="g-row col">
          <div>
            <div className="info-label">Never attach these paths</div>
            <div className="info-desc">
              Files matching these patterns are refused <strong>before being read</strong> —{" "}
              <code>.env</code>, <code>*.pem</code>, <code>~/.ssh/*</code> and anything you add.
              One pattern per line.
            </div>
          </div>
          <label className="field">
            <span className="field-label" style={{ display: "none" }}>
              Excluded paths
            </span>
            <textarea
              value={draft}
              onChange={(e) => {
                setDraft(e.target.value);
                setDirty(true);
              }}
              spellCheck={false}
              aria-label="Excluded paths (one per line)"
              data-testid="excluded-paths"
            />
          </label>
          <div className="settings-actions">
            <button
              type="button"
              className="btn primary"
              onClick={() => void save()}
              disabled={!dirty || saving}
            >
              {saving ? "Saving…" : "Save exclusions"}
            </button>
          </div>
        </div>
      </div>

      <div className="settings-section">Context &amp; data</div>
      <div className="settings-group">
        <div className="g-row">
          <span className="trust-cue">
            <Icon name="shield" size={15} />
            <span>
              <strong>Runs locally.</strong> Handover binds to 127.0.0.1, checks exclusions
              before reading files, caps everything at 1 MiB, and never uploads your context.
            </span>
          </span>
        </div>
      </div>

      <div className="settings-section">History</div>
      <div className="settings-group">
        <div className="g-row">
          <div>
            <div className="info-label">Session history</div>
            <div className="info-desc">
              Recent handoffs live in memory while Handover runs — quitting the app clears them.
            </div>
          </div>
          <button
            type="button"
            className={`btn danger${confirmClear ? " armed" : ""}`}
            onClick={() => (confirmClear ? void clear() : setConfirmClear(true))}
            data-testid="clear-history"
          >
            {confirmClear ? "Confirm?" : "Clear history"}
          </button>
        </div>
      </div>
    </>
  );
}
