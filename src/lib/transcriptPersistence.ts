import type { TranscriptSegment } from "./types";

/**
 * What was last saved of each segment (by id) in this window. A line keeps
 * growing after it is first saved (the STT merges continuous speech into
 * one line, re-sent under the same id), so saving is by content, not only
 * by position.
 */
const savedSignatures = new Map<string, string>();

function signature(segment: TranscriptSegment): string {
  return `${segment.text}\u0000${segment.speaker_tag ?? ""}`;
}

/**
 * Segments to save now, and the new checkpoint index.
 * - Lines at or after `lastIndex`: the contiguous run of finals (all finals
 *   with `all`, at meeting end); an open interim stops the run so it is
 *   not skipped when it finalizes.
 * - Lines before the new checkpoint whose text or speaker tag changed since
 *   they were saved (or were never saved by this window).
 */
export function segmentsToSave(
  segments: TranscriptSegment[],
  lastIndex: number,
  { all = false }: { all?: boolean } = {}
): { toSave: TranscriptSegment[]; nextIndex: number } {
  let nextIndex = Math.min(lastIndex, segments.length);
  if (all) {
    nextIndex = segments.length;
  } else {
    while (nextIndex < segments.length && segments[nextIndex].is_final) nextIndex += 1;
  }
  const toSave = segments
    .slice(0, nextIndex)
    .filter((s) => s.is_final && savedSignatures.get(s.id) !== signature(s));
  return { toSave, nextIndex };
}

/** Record that `segment` was saved as it is now. */
export function markSegmentSaved(segment: TranscriptSegment): void {
  savedSignatures.set(segment.id, signature(segment));
}

/** The IPC form of a segment to save (frontend-only fields left out). */
export function segmentForSave(segment: TranscriptSegment): string {
  const { pending_text: _pending, ...saved } = segment;
  return JSON.stringify(saved);
}

/** Forget what was saved (a new meeting starts). */
export function resetSavedSegments(): void {
  savedSignatures.clear();
}
