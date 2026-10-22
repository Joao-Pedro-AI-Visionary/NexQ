import { useState, useCallback, useRef, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  onStreamToken,
  onStreamEnd,
  onStreamError,
} from "../lib/events";
import { generateAssist, updateMeetingSummary } from "../lib/ipc";
import { assistSegments } from "../lib/assistSegments";
import { useMeetingStore, pastMeetingIdFor } from "../stores/meetingStore";
import type { Meeting } from "../lib/types";

export interface SummaryGenerationState {
  isGenerating: boolean;
  streamedContent: string;
  error: string | null;
  generate: () => Promise<void>;
  cancel: () => void;
}

export function useSummaryGeneration(
  meeting: Meeting | null,
  onSummaryGenerated?: (summary: string) => void
): SummaryGenerationState {
  const [isGenerating, setIsGenerating] = useState(false);
  const [streamedContent, setStreamedContent] = useState("");
  const [error, setError] = useState<string | null>(null);
  const contentRef = useRef("");
  // request_id of the generation this hook started (null when idle/cancelled)
  const requestIdRef = useRef<string | null>(null);
  const unlistenersRef = useRef<(() => void)[]>([]);

  // Cleanup listeners on unmount
  useEffect(() => {
    return () => {
      unlistenersRef.current.forEach((fn) => fn());
      unlistenersRef.current = [];
    };
  }, []);

  const generate = useCallback(async () => {
    if (!meeting || isGenerating) return;

    setIsGenerating(true);
    setStreamedContent("");
    setError(null);
    contentRef.current = "";
    const requestId = crypto.randomUUID();
    requestIdRef.current = requestId;
    const isOurs = (id: string | null) =>
      id === requestId && requestIdRef.current === requestId;

    // Subscribe to stream events before triggering generation
    const cleanups: (() => void)[] = [];

    try {
      const unToken = await onStreamToken((event) => {
        if (!isOurs(event.request_id)) return;
        contentRef.current += event.token;
        setStreamedContent(contentRef.current);
      });
      cleanups.push(unToken);

      const unEnd = await onStreamEnd(async (event) => {
        if (!isOurs(event.request_id)) return;
        requestIdRef.current = null;

        // Persist summary to DB and refresh sidebar
        // A stopped stream is a normal stop: its partial output is not parsed or saved.
        if (!event.cancelled && meeting && contentRef.current) {
          try {
            await updateMeetingSummary(meeting.id, contentRef.current);
            onSummaryGenerated?.(contentRef.current);
            // Refresh sidebar so has_summary badge updates
            useMeetingStore.getState().loadRecentMeetings();
          } catch (err) {
            console.error("[summaryGeneration] Failed to persist summary:", err);
          }
        }

        setIsGenerating(false);
        // Cleanup listeners
        cleanups.forEach((fn) => fn());
        unlistenersRef.current = unlistenersRef.current.filter(
          (fn) => !cleanups.includes(fn)
        );
      });
      cleanups.push(unEnd);

      const unError = await onStreamError((event) => {
        if (!isOurs(event.request_id)) return;
        requestIdRef.current = null;
        setError(event.message);
        setIsGenerating(false);
        cleanups.forEach((fn) => fn());
        unlistenersRef.current = unlistenersRef.current.filter(
          (fn) => !cleanups.includes(fn)
        );
      });
      cleanups.push(unError);

      unlistenersRef.current.push(...cleanups);

      // Pass the meeting's transcript directly to bypass empty transcriptStore
      // (diarized lines labelled with their person's name)
      const segments = assistSegments(
        meeting.transcript.filter((s) => s.is_final),
        false,
        meeting.speakers
      );

      await generateAssist("MeetingSummary", undefined, {
        requestId,
        transcriptSegments: segments,
        pastMeetingId: pastMeetingIdFor(meeting),
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
      setIsGenerating(false);
      cleanups.forEach((fn) => fn());
    }
  }, [meeting, isGenerating, onSummaryGenerated]);

  const cancel = useCallback(async () => {
    try {
      await invoke("cancel_generation");
    } catch {
      // Non-critical
    }
    requestIdRef.current = null;
    setIsGenerating(false);
  }, []);

  return {
    isGenerating,
    streamedContent,
    error,
    generate,
    cancel,
  };
}
