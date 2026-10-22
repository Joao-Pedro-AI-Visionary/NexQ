import type { AssistTranscriptSegment, SpeakerIdentity } from "./types";
import { assistSpeakerLabel } from "./speakerLabels";

type FinalSegment = Pick<AssistTranscriptSegment, "text" | "speaker" | "timestamp_ms"> & {
  id: string;
  speaker_id?: string;
  speaker_tag?: string;
};

/**
 * Final transcript segments for a meeting-view `generate_assist` request.
 *
 * With `mergeRuns`, consecutive same-speaker segments become one block
 * (keeping the first id) so a stateless prompt stays small. A Claude CLI
 * session tracks what it was sent by segment id, so for it every segment
 * must go on its own with its own id: a merged block would mark one id sent
 * while carrying other segments' text (duplicated or dropped later).
 *
 * A diarized line is labelled with its person's name from `speakers` (the
 * meeting's saved speakers), else "Speaker N", so the model can attribute
 * statements; other lines keep their speaker.
 */
export function assistSegments(
  finals: FinalSegment[],
  mergeRuns: boolean,
  speakers?: SpeakerIdentity[],
): AssistTranscriptSegment[] {
  const names = new Map((speakers ?? []).map((s) => [s.id, s.display_name]));
  const nameOf = (key: string) => names.get(key);
  const labelled = finals.map((s) => ({
    id: s.id,
    text: s.text,
    speaker: assistSpeakerLabel(s, nameOf),
    timestamp_ms: s.timestamp_ms,
  }));
  if (!mergeRuns) {
    return labelled;
  }
  const merged: (AssistTranscriptSegment & { id: string })[] = [];
  for (const seg of labelled) {
    const last = merged.length > 0 ? merged[merged.length - 1] : null;
    if (last && last.speaker === seg.speaker) {
      last.text += " " + seg.text;
    } else {
      merged.push({ ...seg });
    }
  }
  return merged;
}
