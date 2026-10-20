import { create } from "zustand";
import type { AgentSessionStatus, ClaudeCliStatus } from "../lib/types";
import { claudeCliStatus, getAgentSessions } from "../lib/ipc";
import { onAgentSessionStatus } from "../lib/events";

/** Map key of a session: `${scope}:${meeting_id ?? ""}:${tier}`. */
export function agentSessionKey(s: Pick<AgentSessionStatus, "scope" | "meeting_id" | "tier">): string {
  return `${s.scope}:${s.meeting_id ?? ""}:${s.tier}`;
}

/** A session that holds (or is about to hold) a running `claude` process. */
export function isSessionRunning(s: AgentSessionStatus): boolean {
  return s.state === "starting" || s.state === "ready" || s.state === "busy";
}

interface AgentState {
  /** Last `claude_cli_status` answer; undefined until the first check ends. */
  status?: ClaudeCliStatus;
  /** A status check is in flight (it spawns child processes: a few seconds). */
  checking: boolean;
  /** Why the last status check failed; cleared by the next successful one. */
  statusError?: string;
  /** Open Claude sessions by `agentSessionKey`. Closed sessions are dropped. */
  sessions: Record<string, AgentSessionStatus>;
  /** Re-detect the CLI and its login. Concurrent calls share one check. */
  refreshStatus: () => Promise<void>;
  /** Subscribe to `agent_session_status` and load the current sessions (once per window). */
  init: () => void;
}

let inFlight: Promise<void> | null = null;
let initialized = false;

function applySession(
  sessions: Record<string, AgentSessionStatus>,
  status: AgentSessionStatus,
): Record<string, AgentSessionStatus> {
  const key = agentSessionKey(status);
  if (status.state === "closed") {
    if (!(key in sessions)) return sessions;
    const next = { ...sessions };
    delete next[key];
    return next;
  }
  return { ...sessions, [key]: status };
}

export const useAgentStore = create<AgentState>((set) => ({
  status: undefined,
  checking: false,
  sessions: {},

  refreshStatus: () => {
    if (inFlight) return inFlight;
    set({ checking: true });
    inFlight = claudeCliStatus()
      .then((status) => set({ status, statusError: undefined }))
      .catch((e) => {
        console.warn("[agentStore] Claude CLI status failed:", e);
        set({ statusError: e instanceof Error ? e.message : String(e) });
      })
      .finally(() => {
        inFlight = null;
        set({ checking: false });
      });
    return inFlight;
  },

  init: () => {
    if (initialized) return;
    initialized = true;
    onAgentSessionStatus((status) => {
      set((s) => ({ sessions: applySession(s.sessions, status) }));
    }).catch((e) => console.warn("[agentStore] Failed to listen for session status:", e));
    getAgentSessions()
      .then((all) =>
        set((s) => {
          // Events that arrived meanwhile are newer than the snapshot.
          const sessions = { ...s.sessions };
          for (const status of all) {
            const key = agentSessionKey(status);
            if (!(key in sessions) && status.state !== "closed") sessions[key] = status;
          }
          return { sessions };
        }),
      )
      .catch((e) => console.warn("[agentStore] Failed to load sessions:", e));
  },
}));

/** Test-only: forget the subscription and any in-flight check. */
export function resetAgentStoreForTests(): void {
  inFlight = null;
  initialized = false;
  useAgentStore.setState({ status: undefined, checking: false, statusError: undefined, sessions: {} });
}
