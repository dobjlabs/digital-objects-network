import { useEffect, useState } from "react";

import {
  getAppSettings,
  saveAppSettings,
  type AppSettingsPayload,
} from "../../shared/api/tauriClient";
import { normalizeErrorMessage } from "../../shared/error";

interface SettingsModalProps {
  open: boolean;
  onClose: () => void;
}

export function SettingsModal({ open, onClose }: SettingsModalProps) {
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [form, setForm] = useState<AppSettingsPayload>({
    synchronizerApiUrl: "",
    relayerApiUrl: "",
  });

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    getAppSettings()
      .then((settings) => {
        if (cancelled) return;
        setForm(settings);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(normalizeErrorMessage(err, "Failed to save settings"));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !saving) {
        event.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, onClose, saving]);

  if (!open) return null;

  const handleSave = async () => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      const next = await saveAppSettings(form);
      setForm(next);
      onClose();
    } catch (err) {
      setError(normalizeErrorMessage(err, "Failed to save settings"));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div
      className="settings-modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !saving) {
          onClose();
        }
      }}
    >
      <section
        className="settings-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
      >
        <header className="settings-modal-header">
          <h2>Settings</h2>
        </header>

        <div className="settings-modal-body">
          <label className="settings-field">
            <span>SYNCHRONIZER_API_URL</span>
            <input
              type="text"
              value={form.synchronizerApiUrl}
              onChange={(event) =>
                setForm((prev) => ({
                  ...prev,
                  synchronizerApiUrl: event.target.value,
                }))
              }
              disabled={loading || saving}
              spellCheck={false}
            />
          </label>

          <label className="settings-field">
            <span>RELAYER_API_URL</span>
            <input
              type="text"
              value={form.relayerApiUrl}
              onChange={(event) =>
                setForm((prev) => ({
                  ...prev,
                  relayerApiUrl: event.target.value,
                }))
              }
              disabled={loading || saving}
              spellCheck={false}
            />
          </label>

          {error && <div className="settings-error">{error}</div>}
        </div>

        <footer className="settings-modal-footer">
          <button
            type="button"
            className="settings-btn secondary"
            onClick={onClose}
            disabled={saving}
          >
            Cancel
          </button>
          <button
            type="button"
            className="settings-btn primary"
            onClick={() => void handleSave()}
            disabled={loading || saving}
          >
            {saving ? "Saving..." : "Save"}
          </button>
        </footer>
      </section>
    </div>
  );
}
