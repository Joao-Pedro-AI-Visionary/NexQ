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
