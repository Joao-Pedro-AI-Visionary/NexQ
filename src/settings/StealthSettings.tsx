import { useEffect, useState } from "react";
import { ScreenShareOff } from "lucide-react";
import { DEFAULT_STEALTH_SHORTCUT, useConfigStore } from "../stores/configStore";
import { showToast } from "../stores/toastStore";
import { getStealthSupport } from "../lib/ipc";
import { acceleratorFromEvent, formatShortcut } from "../lib/shortcutLabel";
import type { StealthSupport } from "../lib/types";

/** Settings → General: stealth mode ("Hide from screen share") and its shortcut. */
export function StealthSettings() {
  const stealthMode = useConfigStore((s) => s.stealthMode);
  const setStealthMode = useConfigStore((s) => s.setStealthMode);
  const shortcut = useConfigStore((s) => s.stealthShortcut);
  const setShortcut = useConfigStore((s) => s.setStealthShortcut);

  const [support, setSupport] = useState<StealthSupport | null>(null);
  const [busy, setBusy] = useState(false);
  const [recording, setRecording] = useState(false);

  useEffect(() => {
    getStealthSupport().then(setSupport).catch(() => setSupport(null));
  }, []);

  // Rebinding: the next Ctrl/Cmd/Alt combination becomes the shortcut; Esc cancels.
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setRecording(false);
        return;
      }
      const accel = acceleratorFromEvent(e);
      if (!accel) return;
      setShortcut(accel);
      setRecording(false);
      showToast(`Shortcut set to ${formatShortcut(accel)}`, "success");
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [recording, setShortcut]);

  const unsupported = support?.supported === false;

  const toggle = async () => {
    setBusy(true);
    try {
      await setStealthMode(!stealthMode);
    } catch (e) {
      showToast(`Couldn't change screen-share visibility: ${e instanceof Error ? e.message : String(e)}`, "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="rounded-xl border border-border/30 bg-card/50 p-5 space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div className="flex gap-2.5">
          <ScreenShareOff
            className={`mt-0.5 h-4 w-4 shrink-0 transition-colors duration-200 ${stealthMode ? "text-primary" : "text-muted-foreground"}`}
            aria-hidden="true"
          />
          <div>
            <label id="stealth-label" className="text-sm font-medium text-foreground">
              Hide from screen share
            </label>
            <p className="mt-0.5 text-xs text-muted-foreground">
              NexQ stays on your screen but won't appear in screen sharing, recordings or
              screenshots (Zoom, Teams, Meet, OBS and others).
            </p>
          </div>
        </div>
        <button
          onClick={toggle}
          disabled={busy || unsupported}
          role="switch"
          aria-checked={stealthMode}
          aria-labelledby="stealth-label"
          className={`relative h-6 w-11 shrink-0 cursor-pointer rounded-full transition-all duration-200 disabled:cursor-not-allowed disabled:opacity-50 ${
            stealthMode ? "bg-primary shadow-[0_0_0_3px_hsl(var(--primary)/0.12)]" : "bg-muted"
          }`}
        >
          <span
            className={`absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white shadow-sm transition-all duration-200 ${
              stealthMode ? "translate-x-5 scale-[1.05]" : "translate-x-0"
            }`}
          />
        </button>
      </div>

      <div className="h-px bg-border/20" />

      <div className="flex items-center justify-between gap-4">
        <div>
          <span className="text-xs font-medium text-foreground/80">Shortcut</span>
          <p className="mt-0.5 text-xs text-muted-foreground">
            Works from any app. Also in the tray menu and the meeting overlay.
          </p>
        </div>
        <div className="flex items-center gap-2">
          {shortcut !== DEFAULT_STEALTH_SHORTCUT && !recording && (
            <button
              onClick={() => setShortcut(DEFAULT_STEALTH_SHORTCUT)}
              className="rounded-md px-2 py-1 text-meta text-muted-foreground hover:bg-accent hover:text-foreground cursor-pointer"
            >
              Reset
            </button>
          )}
          <button
            onClick={() => setRecording((r) => !r)}
            aria-label={recording ? "Press the new shortcut, or Escape to cancel" : `Change shortcut, currently ${formatShortcut(shortcut)}`}
            className={`rounded-lg px-2.5 py-1 font-mono text-xs transition-colors duration-150 cursor-pointer ${
              recording ? "bg-primary/15 text-primary ring-1 ring-primary/30" : "bg-secondary/60 text-foreground hover:bg-secondary"
            }`}
          >
            {recording ? "Press keys…" : formatShortcut(shortcut)}
          </button>
        </div>
      </div>

      {support?.note && <p className="text-meta text-muted-foreground/70">{support.note}</p>}
    </div>
  );
}
