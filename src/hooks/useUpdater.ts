// Discord-style updates: a single owner (the launcher window) checks 10 s after
// launch and every 4 h, downloads silently when automatic updates are on, and
// the footer pill restarts into the new version. Rust installs a downloaded
// update on quit (see updater_commands::install_on_quit).
//
// The action functions below work from any window (Settings -> About can open
// in the overlay); only the check loop and event listeners are launcher-only.

import { useEffect } from "react";
import { load } from "@tauri-apps/plugin-store";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useUpdaterStore } from "../stores/updaterStore";
import { useMeetingStore } from "../stores/meetingStore";
import { showToast } from "../stores/toastStore";
import {
  checkForUpdate,
  downloadUpdate,
  getPendingUpdate,
  installUpdateNow,
} from "../lib/ipc";
import { onUpdateDownloadProgress, onUpdateDownloaded } from "../lib/events";

// Same keys are read by Rust at quit time — keep in sync with updater_commands.rs.
const STORE_NAME = "nexq-settings.json";
const SKIPPED_VERSION_KEY = "skipped_version";
const AUTO_UPDATE_KEY = "auto_update";

const STARTUP_DELAY_MS = 10_000;
const CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000; // 4 hours

export const RELEASES_URL = "https://github.com/naxhq/NexQ/releases";

export function releaseNotesUrl(version: string): string {
  return `${RELEASES_URL}/tag/v${version}`;
}

function errorMessage(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return "Unknown error";
}

function settingsStore() {
  return load(STORE_NAME, { autoSave: true, defaults: {} });
}

async function persist(key: string, value: unknown): Promise<void> {
  try {
    const store = await settingsStore();
    if (value === undefined) await store.delete(key);
    else await store.set(key, value);
  } catch (err) {
    console.warn(`[updater] Failed to persist ${key}:`, err);
  }
}

/** Load persisted preferences and any update already downloaded by Rust. */
export async function hydrateUpdater(): Promise<void> {
  const updater = useUpdaterStore.getState();
  try {
    const store = await settingsStore();
    const skipped = await store.get<string>(SKIPPED_VERSION_KEY);
    const auto = await store.get<boolean>(AUTO_UPDATE_KEY);
    updater.setSkippedVersion(skipped ?? undefined);
    if (typeof auto === "boolean") updater.setAutoUpdate(auto);
  } catch (err) {
    console.warn("[updater] Failed to load preferences:", err);
  }

  try {
    const pending = await getPendingUpdate();
    const { skippedVersion } = useUpdaterStore.getState();
    if (pending && pending.version !== skippedVersion) {
      updater.setReady(pending.version, pending.body);
    }
  } catch (err) {
    console.warn("[updater] Failed to read pending update:", err);
  }
}

/**
 * Check for an update; download it when automatic updates are on.
 * Automatic checks ignore a skipped version, manual checks don't.
 */
export async function checkForUpdates(opts?: { manual?: boolean }): Promise<void> {
  const { phase, startCheck } = useUpdaterStore.getState();
  if (phase === "checking" || phase === "downloading" || phase === "ready") return;

  startCheck();
  let update;
  try {
    const found = await checkForUpdate();
    update = useUpdaterStore.getState().finishCheck(found, opts);
  } catch (err) {
    useUpdaterStore.getState().failCheck(errorMessage(err));
    return;
  }

  if (update && useUpdaterStore.getState().autoUpdate) {
    await downloadUpdateNow();
  }
}

/** Download the available update in the background (no install). */
export async function downloadUpdateNow(): Promise<void> {
  const { phase, startDownload } = useUpdaterStore.getState();
  if (phase === "downloading" || phase === "ready") return;

  startDownload();
  try {
    const info = await downloadUpdate();
    useUpdaterStore.getState().setReady(info.version, info.body);
  } catch (err) {
    useUpdaterStore.getState().failDownload(errorMessage(err));
  }
}

/**
 * Install the downloaded update now; NexQ exits and relaunches on the new
 * version. If a meeting is running, confirm first and end it cleanly.
 */
export async function restartToUpdate(): Promise<void> {
  if (useMeetingStore.getState().activeMeeting) {
    const { confirm } = await import("@tauri-apps/plugin-dialog");
    const ok = await confirm(
      "Your meeting will be saved, then NexQ restarts on the new version.",
      {
        title: "End meeting and update?",
        kind: "warning",
        okLabel: "End & update",
        cancelLabel: "Cancel",
      },
    );
    if (!ok) return;
    await useMeetingStore.getState().endMeetingFlow();
  }

  try {
    await installUpdateNow();
  } catch (err) {
    showToast(`Update failed: ${errorMessage(err)}`, "error");
  }
}

export async function setAutoUpdate(enabled: boolean): Promise<void> {
  useUpdaterStore.getState().setAutoUpdate(enabled);
  await persist(AUTO_UPDATE_KEY, enabled);
}

/**
 * Skip a version: automatic checks stop offering it and Rust won't install it
 * on quit. A manual check still finds it.
 */
export async function skipVersion(version: string): Promise<void> {
  const updater = useUpdaterStore.getState();
  updater.setSkippedVersion(version);
  if (updater.version === version) {
    useUpdaterStore.setState({ phase: "idle", version: undefined, notes: undefined, error: undefined });
  }
  await persist(SKIPPED_VERSION_KEY, version);
}

/** Runs the update loop. Mount once at the app root; active only in the launcher window. */
export function useUpdater(): void {
  useEffect(() => {
    let isLauncher = false;
    try {
      isLauncher = getCurrentWebviewWindow().label === "launcher";
    } catch {
      isLauncher = false;
    }
    if (!isLauncher) return;

    let disposed = false;
    const unlisteners: UnlistenFn[] = [];
    const keep = (fn: UnlistenFn) => {
      if (disposed) fn();
      else unlisteners.push(fn);
    };

    hydrateUpdater();

    onUpdateDownloadProgress((e) => {
      useUpdaterStore.getState().addProgress(e.chunk_length, e.content_length);
    }).then(keep);

    // Fires for downloads started from any window (e.g. About in the overlay).
    onUpdateDownloaded((info) => {
      useUpdaterStore.getState().setReady(info.version, info.body);
    }).then(keep);

    const startupTimer = setTimeout(() => checkForUpdates(), STARTUP_DELAY_MS);
    const intervalId = setInterval(() => checkForUpdates(), CHECK_INTERVAL_MS);

    return () => {
      disposed = true;
      unlisteners.forEach((fn) => fn());
      clearTimeout(startupTimer);
      clearInterval(intervalId);
    };
  }, []);
}
