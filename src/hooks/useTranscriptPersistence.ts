import { useEffect, useRef } from "react";
import { useMeetingStore } from "../stores/meetingStore";
import { useTranscriptStore } from "../stores/transcriptStore";
import { appendTranscriptSegment } from "../lib/ipc";
import { markSegmentSaved, segmentForSave, segmentsToSave } from "../lib/transcriptPersistence";

const FLUSH_INTERVAL_MS = 30_000; // 30 seconds

/**
 * Hook that persists transcript segments to SQLite incrementally.
 * Every 30 seconds during an active meeting, it flushes any new
 * (not-yet-persisted) transcript segments to the database, and saves again
 * lines that grew since (same id, longer text).
 */
export function useTranscriptPersistence() {
  const activeMeeting = useMeetingStore((s) => s.activeMeeting);
  const lastPersistedIndex = useMeetingStore((s) => s.lastPersistedIndex);
  const setLastPersistedIndex = useMeetingStore((s) => s.setLastPersistedIndex);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    if (!activeMeeting) {
      // No active meeting — clear interval
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
      return;
    }

    const meetingId = activeMeeting.id;

    async function flushSegments() {
      const segments = useTranscriptStore.getState().segments;
      const currentLastIndex = useMeetingStore.getState().lastPersistedIndex;

      // Persist the contiguous finalized prefix after the last checkpoint
      // (an open interim stops it, so it isn't skipped when it finalizes),
      // plus earlier lines that grew or changed since they were saved.
      const { toSave, nextIndex } = segmentsToSave(segments, currentLastIndex);

      for (const segment of toSave) {
        try {
          await appendTranscriptSegment(meetingId, segmentForSave(segment));
          markSegmentSaved(segment);
        } catch (err) {
          console.error("[transcriptPersistence] Failed to persist segment:", err);
          // Stop trying to persist more if one fails
          return;
        }
      }

      if (nextIndex !== currentLastIndex) setLastPersistedIndex(nextIndex);
    }

    // Set up the 30-second interval
    intervalRef.current = setInterval(flushSegments, FLUSH_INTERVAL_MS);

    return () => {
      // On cleanup, do a final flush
      flushSegments();

      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
  }, [activeMeeting, setLastPersistedIndex]);
}
