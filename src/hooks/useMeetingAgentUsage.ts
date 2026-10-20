import { useMemo } from "react";
import { useAgentStore } from "../stores/agentStore";
import { useMeetingStore } from "../stores/meetingStore";
import { liveSessionsOf, scopeUsage } from "../lib/agentUsage";
import type { AgentSessionStatus, SessionUsage } from "../lib/types";

/** Live sessions of the active meeting, and their cumulative usage. */
export function useMeetingAgentUsage(): { sessions: AgentSessionStatus[]; usage: SessionUsage } {
  const meetingId = useMeetingStore((s) => s.activeMeeting?.id ?? null);
  const all = useAgentStore((s) => s.sessions);
  return useMemo(() => {
    const sessions = liveSessionsOf(all, meetingId);
    return { sessions, usage: scopeUsage(sessions) };
  }, [all, meetingId]);
}
