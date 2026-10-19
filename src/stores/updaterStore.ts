import { create } from "zustand";
import type { UpdateInfo } from "../lib/types";

/**
 * Updater lifecycle. An update that was found but not downloaded yet (automatic
 * updates off) is `phase: "idle"` with `version` set.
 */
export type UpdatePhase = "idle" | "checking" | "downloading" | "ready" | "error";

export interface UpdateProgress {
  received: number;
  total: number | null;
}

interface UpdaterState {
  phase: UpdatePhase;
  /** Version of the available / downloading / ready update. */
  version?: string;
  /** Release notes of that update. */
  notes?: string;
  progress: UpdateProgress;
  error?: string;
  lastChecked?: number;

  // Persisted preferences (hydrated by useUpdater)
  autoUpdate: boolean;
  skippedVersion?: string;

  startCheck: () => void;
  /**
   * Record a finished check. Returns the update to act on, or null when there
   * is none or it was skipped (automatic checks only — a manual check always
   * surfaces the update).
   */
  finishCheck: (update: UpdateInfo | null, opts?: { manual?: boolean }) => UpdateInfo | null;
  failCheck: (error: string) => void;
  startDownload: () => void;
  addProgress: (chunkLength: number, contentLength: number | null) => void;
  setReady: (version: string, notes?: string | null) => void;
  failDownload: (error: string) => void;
  setAutoUpdate: (enabled: boolean) => void;
  setSkippedVersion: (version: string | undefined) => void;
  reset: () => void;
}

const initialState = {
  phase: "idle" as UpdatePhase,
  version: undefined as string | undefined,
  notes: undefined as string | undefined,
  progress: { received: 0, total: null } as UpdateProgress,
  error: undefined as string | undefined,
  lastChecked: undefined as number | undefined,
};

export const useUpdaterStore = create<UpdaterState>((set, get) => ({
  ...initialState,
  autoUpdate: true,
  skippedVersion: undefined,

  startCheck: () => set({ phase: "checking", error: undefined }),

  finishCheck: (update, opts) => {
    const skipped =
      !!update && !opts?.manual && get().skippedVersion === update.version;
    const actionable = update && !skipped ? update : null;
    set({
      phase: "idle",
      error: undefined,
      lastChecked: Date.now(),
      version: actionable?.version,
      notes: actionable?.body ?? undefined,
    });
    return actionable;
  },

  failCheck: (error) => set({ phase: "error", error, version: undefined, notes: undefined }),

  startDownload: () =>
    set({ phase: "downloading", error: undefined, progress: { received: 0, total: null } }),

  addProgress: (chunkLength, contentLength) =>
    set((s) => ({
      phase: "downloading",
      progress: {
        received: s.progress.received + chunkLength,
        total: contentLength ?? s.progress.total,
      },
    })),

  setReady: (version, notes) =>
    set((s) => ({
      phase: "ready",
      version,
      notes: notes ?? (s.version === version ? s.notes : undefined),
      error: undefined,
    })),

  failDownload: (error) => set({ phase: "error", error }),

  setAutoUpdate: (enabled) => set({ autoUpdate: enabled }),

  setSkippedVersion: (version) => set({ skippedVersion: version }),

  reset: () => set({ ...initialState }),
}));
