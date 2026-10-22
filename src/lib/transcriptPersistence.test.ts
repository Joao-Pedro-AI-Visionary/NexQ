import { beforeEach, describe, expect, it } from "vitest";
import { markSegmentSaved, resetSavedSegments, segmentForSave, segmentsToSave } from "./transcriptPersistence";
import type { TranscriptSegment } from "./types";

function seg(id: string, text: string, is_final = true, speaker_tag?: string): TranscriptSegment {
  return { id, text, speaker: "Them", speaker_id: "them", speaker_tag, timestamp_ms: 1, is_final, confidence: 1 };
}

function save(segments: TranscriptSegment[], lastIndex: number, all = false) {
  const { toSave, nextIndex } = segmentsToSave(segments, lastIndex, { all });
  toSave.forEach(markSegmentSaved);
  return { ids: toSave.map((s) => `${s.id}:${s.text}`), nextIndex };
}

describe("transcript persistence", () => {
  beforeEach(() => resetSavedSegments());

  it("saves new finals up to an open interim", () => {
    const segments = [seg("a", "one"), seg("b", "two"), seg("c", "thr", false), seg("d", "four")];
    expect(save(segments, 0)).toEqual({ ids: ["a:one", "b:two"], nextIndex: 2 });
    // At meeting end every final goes.
    expect(save(segments, 2, true)).toEqual({ ids: ["d:four"], nextIndex: 4 });
  });

  it("saves a line again when it grew after it was saved, once", () => {
    let segments = [seg("a", "We ship"), seg("b", "Ok")];
    expect(save(segments, 0).nextIndex).toBe(2);
    segments = [seg("a", "We ship on Friday"), seg("b", "Ok")];
    expect(save(segments, 2)).toEqual({ ids: ["a:We ship on Friday"], nextIndex: 2 });
    expect(save(segments, 2).ids).toEqual([]);
    // A changed speaker tag is saved too.
    segments = [seg("a", "We ship on Friday", true, "S2"), seg("b", "Ok")];
    expect(save(segments, 2).ids).toEqual(["a:We ship on Friday"]);
  });

  it("does not save frontend-only fields", () => {
    const json = JSON.parse(segmentForSave({ ...seg("a", "Hi", true, "S1"), pending_text: "Hi the" }));
    expect(json.pending_text).toBeUndefined();
    expect(json.speaker_tag).toBe("S1");
  });
});
