/** True on macOS (shortcuts show ⌘ ⇧ ⌥ ⌃ there). */
export function isMacPlatform(): boolean {
  return typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);
}

const MAC_SYMBOLS: Record<string, string> = {
  CmdOrCtrl: "⌘",
  CommandOrControl: "⌘",
  Cmd: "⌘",
  Command: "⌘",
  Super: "⌘",
  Meta: "⌘",
  Ctrl: "⌃",
  Control: "⌃",
  Shift: "⇧",
  Alt: "⌥",
  Option: "⌥",
};

/**
 * A Tauri accelerator ("CmdOrCtrl+Shift+H") as the user reads it:
 * "Ctrl+Shift+H" on Windows, "⌘⇧H" on macOS.
 */
export function formatShortcut(accelerator: string, mac = isMacPlatform()): string {
  const parts = accelerator.split("+").filter(Boolean);
  if (mac) return parts.map((p) => MAC_SYMBOLS[p] ?? p).join("");
  return parts
    .map((p) => (p === "CmdOrCtrl" || p === "CommandOrControl" ? "Ctrl" : p === "Super" || p === "Meta" ? "Win" : p))
    .join("+");
}

/**
 * The accelerator for a pressed key combination, or null for a lone modifier
 * or a combination without Ctrl/Cmd/Alt (global shortcuts need one).
 * Ctrl on Windows and Cmd on macOS both become "CmdOrCtrl".
 */
export function acceleratorFromEvent(e: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">): string | null {
  if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return null;
  if (!e.ctrlKey && !e.metaKey && !e.altKey) return null;
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("CmdOrCtrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  // Use the physical key so Shift/Alt don't turn "H" into a symbol.
  let key: string;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit[0-9]$/.test(e.code)) key = e.code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(e.code)) key = e.code;
  else return null;
  parts.push(key);
  return parts.join("+");
}
