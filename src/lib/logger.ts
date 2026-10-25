// Frontend errors and warnings go to the app log file too (Settings → About →
// Diagnostics), so a bug report's log covers both halves of the app.
// Rate-limited and de-duplicated: a noisy loop can't flood the file.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

export type LogLevel = "error" | "warn" | "info";

/** Most lines forwarded per minute; the rest are counted and summarized. */
const MAX_PER_MINUTE = 60;

let windowStart = 0;
let sentThisWindow = 0;
let suppressed = 0;
let lastMessage = "";
let repeats = 0;

function describe(arg: unknown): string {
  if (arg instanceof Error) return arg.stack ?? `${arg.name}: ${arg.message}`;
  if (typeof arg === "string") return arg;
  try {
    return JSON.stringify(arg) ?? String(arg);
  } catch {
    return String(arg);
  }
}

function send(level: LogLevel, source: string, message: string) {
  invoke("write_frontend_log", { level, source, message }).catch(() => {});
}

/** Write one line to the app log. */
export function logToFile(level: LogLevel, source: string, message: string) {
  if (message === lastMessage) {
    repeats += 1;
    return;
  }
  if (repeats > 0) send(level, source, `(previous message repeated ${repeats} more times)`);
  lastMessage = message;
  repeats = 0;

  const now = Date.now();
  if (now - windowStart > 60_000) {
    if (suppressed > 0) send("warn", source, `${suppressed} log lines skipped (rate limit)`);
    windowStart = now;
    sentThisWindow = 0;
    suppressed = 0;
  }
  if (sentThisWindow >= MAX_PER_MINUTE) {
    suppressed += 1;
    return;
  }
  sentThisWindow += 1;
  send(level, source, message);
}

let installed = false;

/** Forward console errors/warnings and uncaught errors from this window. */
export function installFrontendLogging() {
  if (installed || !("__TAURI_INTERNALS__" in window)) return;
  installed = true;
  let source = "app";
  try {
    source = getCurrentWebviewWindow().label;
  } catch {
    // keep "app"
  }

  for (const level of ["error", "warn"] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      original(...args);
      logToFile(level, source, args.map(describe).join(" "));
    };
  }
  window.addEventListener("error", (e) => {
    const where = e.filename ? ` (${e.filename}:${e.lineno})` : "";
    logToFile("error", source, `Uncaught ${e.error ? describe(e.error) : e.message}${where}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    logToFile("error", source, `Unhandled rejection: ${describe(e.reason)}`);
  });
}
