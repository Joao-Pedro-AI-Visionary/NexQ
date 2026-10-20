import type { AssistTranscriptSegment } from "./types";

type FinalSegment = Pick<AssistTranscriptSegment, "text" | "speaker" | "timestamp_ms"> & { id: string };

/**
 * Final transcript segments for a meeting-view `generate_assist` request.
 *
 * With `mergeRuns`, consecutive same-speaker segments become one block
 * (keeping the first id) so a stateless prompt stays small. A Claude CLI
 * session tracks what it was sent by segment id, so for it every segment
 * must go on its own with its own id: a merged block would mark one id sent
 * while carrying other segments' text (duplicated or dropped later).
 */
export function assistSegments(finals: FinalSegment[], mergeRuns: boolean): AssistTranscriptSegment[] {
  if (!mergeRuns) {
    return finals.map((s) => ({ id: s.id, text: s.text, speaker: s.speaker, timestamp_ms: s.timestamp_ms }));
  }
  const merged: (AssistTranscriptSegment & { id: string })[] = [];
  for (const seg of finals) {
    const last = merged.length > 0 ? merged[merged.length - 1] : null;
    if (last && last.speaker === seg.speaker) {
      last.text += " " + seg.text;
    } else {
      merged.push({ id: seg.id, text: seg.text, speaker: seg.speaker, timestamp_ms: seg.timestamp_ms });
    }
  }
  return merged;
}
