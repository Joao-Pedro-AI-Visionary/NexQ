// ============================================================================
// useCallLogCapture — Subscribes to LLM stream events and records them
// in callLogStore. Now captures ACTUAL prompt data from backend events
// instead of reconstructing from frontend stores.
// ============================================================================

import { useEffect } from "react";
import {
  onStreamStart,
  onStreamToken,
  onStreamEnd,
  onStreamError,
} from "../lib/events";
import { useCallLogStore } from "../stores/callLogStore";
import { useMeetingStore } from "../stores/meetingStore";
import { saveMeetingAiInteractions } from "../lib/ipc";
import type { IntelligenceMode, LogEntry } from "../lib/types";

/** Timeout (ms) for stale "sending" entries — auto-transition to error */
const SENDING_TIMEOUT_MS = 10_000;

export function useCallLogCapture() {
  useEffect(() => {
    const unlisteners: Array<Promise<() => void>> = [];
    // request_id → log entry id, for streams whose start event we saw.
    const entryIds = new Map<string, string>();
    // request_id → "sending" safety timeout
    const timers = new Map<string, ReturnType<typeof setTimeout>>();

    const clearSendingTimeout = (requestId: string) => {
      const timer = timers.get(requestId);
      if (timer) {
        clearTimeout(timer);
        timers.delete(requestId);
      }
    };

    unlisteners.push(
      onStreamStart((event) => {
        const requestId = event.request_id;
        if (!requestId) return;
        const id = crypto.randomUUID();
        entryIds.set(requestId, id);

        const mode = event.mode as IntelligenceMode;

        const entry: LogEntry = {
          id,
          timestamp: Date.now(),
          mode,
          provider: event.provider,
          model: event.model,
          status: "sending",
          startedAt: Date.now(),
          firstTokenAt: null,
          completedAt: null,
          totalTokens: null,
          latencyMs: null,
          responseContent: "",
          responseContentClean: "",
          // Actual prompt data from backend
          actualSystemPrompt: event.system_prompt,
          actualUserPrompt: event.user_prompt,
          // Context source flags
          includeTranscript: event.include_transcript,
          includeRag: event.include_rag,
          includeInstructions: event.include_instructions,
          includeQuestion: event.include_question,
          // Enriched metadata
          temperature: event.temperature ?? null,
          ragQuery: event.rag_query ?? null,
          ragChunks: event.rag_chunks ?? [],
          ragChunksFiltered: event.rag_chunks_filtered ?? 0,
          ragTotalCandidates: event.rag_total_candidates ?? 0,
          transcriptWindowSeconds: event.transcript_window_seconds ?? null,
          transcriptSegmentsCount: event.transcript_segments_count ?? null,
          transcriptSegmentsTotal: event.transcript_segments_total ?? null,
          // Legacy fields (empty for new entries)
          snapshotTranscript: "",
          snapshotContext: "",
          reconstructedSystemPrompt: "",
          errorMessage: null,
        };

        useCallLogStore.getState().beginEntry(entry);

        // Safety timeout: if entry stays in "sending" for >10s, mark as error
        clearSendingTimeout(requestId);
        timers.set(
          requestId,
          setTimeout(() => {
            timers.delete(requestId);
            const entries = useCallLogStore.getState().entries;
            const e = entries.find((x) => x.id === id);
            if (e && e.status === "sending") {
              useCallLogStore
                .getState()
                .failEntry(id, "Timed out waiting for response");
              entryIds.delete(requestId);
            }
          }, SENDING_TIMEOUT_MS)
        );
      })
    );

    unlisteners.push(
      onStreamToken((event) => {
        if (!event.request_id) return;
        const entryId = entryIds.get(event.request_id);
        if (entryId) {
          // Clear sending timeout on first token
          clearSendingTimeout(event.request_id);
          useCallLogStore.getState().appendToken(entryId, event.token);
        }
      })
    );

    unlisteners.push(
      onStreamEnd((event) => {
        if (!event.request_id) return;
        clearSendingTimeout(event.request_id);
        const entryId = entryIds.get(event.request_id);
        if (entryId) {
          entryIds.delete(event.request_id);
          useCallLogStore
            .getState()
            .completeEntry(entryId, event.total_tokens, event.latency_ms);

          // Immediately persist all completed AI interactions to DB (crash protection)
          const meetingId = useMeetingStore.getState().activeMeeting?.id;
          if (meetingId) {
            const entries = useCallLogStore.getState().entries;
            const interactions = entries
              .filter((e) => e.status === "complete")
              .map((e) => ({
                id: e.id,
                meeting_id: meetingId,
                mode: e.mode,
                question_context: e.actualUserPrompt || "",
                response: e.responseContentClean || e.responseContent,
                model: e.model,
                provider: e.provider,
                latency_ms: e.latencyMs ?? 0,
                timestamp: new Date(e.timestamp).toISOString(),
              }));
            if (interactions.length > 0) {
              saveMeetingAiInteractions(
                meetingId,
                JSON.stringify(interactions)
              ).catch((err) =>
                console.error("[callLogCapture] Failed to persist AI interactions:", err)
              );
            }
          }
        }
      })
    );

    unlisteners.push(
      onStreamError((event) => {
        if (!event.request_id) return;
        clearSendingTimeout(event.request_id);
        const entryId = entryIds.get(event.request_id);
        if (entryId) {
          entryIds.delete(event.request_id);
          useCallLogStore.getState().failEntry(entryId, event.message);
        }
      })
    );

    return () => {
      timers.forEach((timer) => clearTimeout(timer));
      timers.clear();
      entryIds.clear();
      unlisteners.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);
}
