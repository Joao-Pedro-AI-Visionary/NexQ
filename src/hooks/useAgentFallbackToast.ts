import { useEffect } from "react";
import { onAgentFallback } from "../lib/events";
import { useMeetingStore } from "../stores/meetingStore";
import { showToast } from "../stores/toastStore";
import { isRateLimitText } from "../components/AgentStatusChip";
import type { AgentFallbackEvent } from "../lib/types";

/** Toast copy for one `agent_fallback` event. */
export function fallbackMessage({ reason, provider }: AgentFallbackEvent): string {
  return isRateLimitText(reason)
    ? `Claude usage limit reached — using ${provider}`
    : `Claude unavailable (${reason}) — using ${provider}`;
}

/**
 * Toast when requests move from Claude to the fallback provider. The backend
 * already announces each outage once. `agent_fallback` reaches both windows,
 * so only the one the user is looking at toasts: the overlay during a meeting,
 * the launcher otherwise (the launcher hides while a meeting runs).
 */
export function useAgentFallbackToast(windowLabel: string): void {
  useEffect(() => {
    if (!windowLabel) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    onAgentFallback((event) => {
      const inMeeting = useMeetingStore.getState().currentView === "overlay";
      if ((windowLabel === "overlay") !== inMeeting) return;
      showToast(fallbackMessage(event), "warning");
    })
      .then((fn) => (cancelled ? fn() : (unlisten = fn)))
      .catch((e) => console.warn("[useAgentFallbackToast] listen failed:", e));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [windowLabel]);
}
