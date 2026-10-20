/**
 * Window switching helpers for the two-window Tauri layout.
 *
 * Keeping show/hide/focus in one place prevents the React view state from
 * drifting away from the actual native windows.
 */
export async function showLauncherWindow(): Promise<void> {
  const { WebviewWindow, getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const current = getCurrentWebviewWindow();
  const launcher = await WebviewWindow.getByLabel("launcher");

  if (launcher) {
    await launcher.show().catch(() => {});
    await launcher.unminimize().catch(() => {});
    await launcher.setFocus().catch(() => {});
  }

  if (current.label === "overlay") {
    await current.hide().catch(() => {});
  }
}

export async function showOverlayWindow(): Promise<void> {
  const { WebviewWindow, getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
  const current = getCurrentWebviewWindow();
  const overlay = await WebviewWindow.getByLabel("overlay");

  if (overlay) {
    await overlay.show().catch(() => {});
    await overlay.unminimize().catch(() => {});
    await overlay.setFocus().catch(() => {});
  }

  if (current.label === "launcher") {
    await current.hide().catch(() => {});
  }
}

/**
 * Speakers, bookmarks, action items and topic sections are edited in the
 * overlay window. When a meeting ends, both windows may run endMeetingFlow
 * (tray Stop, update restart), and saving them replaces all rows — so exactly
 * one window persists them: the overlay, or the launcher only when no overlay
 * window has taken the meeting.
 */
export function shouldPersistOverlayData(windowLabel: string, overlayActive: boolean): boolean {
  if (windowLabel === "overlay") return true;
  return !overlayActive;
}

/**
 * How a window ends the running meeting: by itself, or through every window
 * (the tray's Stop path) when it is not the overlay and the overlay took the
 * meeting — the overlay holds data only it saves (see
 * shouldPersistOverlayData), so it must run its own endMeetingFlow.
 */
export function meetingEndRoute(windowLabel: string, overlayActive: boolean): "here" | "all_windows" {
  return windowLabel !== "overlay" && overlayActive ? "all_windows" : "here";
}

/**
 * What the meeting shortcut (Ctrl+M) does: end the running meeting, start
 * one, or nothing while a deferred quit is pending (NexQ is about to exit).
 */
export function meetingShortcutAction(hasMeeting: boolean, quitPending: boolean): "end" | "start" | "ignore" {
  if (hasMeeting) return "end";
  return quitPending ? "ignore" : "start";
}

/**
 * Whether the user can see a window right now (shown and not minimized).
 * An unknown window or a failed query counts as not shown.
 */
export async function isWindowShown(label: string): Promise<boolean> {
  try {
    const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
    const win = await WebviewWindow.getByLabel(label);
    if (!win) return false;
    return (await win.isVisible()) && !(await win.isMinimized());
  } catch {
    return false;
  }
}
