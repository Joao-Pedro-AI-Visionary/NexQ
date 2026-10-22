import { describe, expect, it } from "vitest";
import { mergeConsecutiveSegments } from "./mergeSegments";
import type { TranscriptSegment } from "./types";

function seg(id: string, ts: number, extra: Partial<TranscriptSegment> = {}): TranscriptSegment {
  return { id, text: id, speaker: "Them", speaker_id: "them", timestamp_ms: ts, is_final: true, confidence: 1, ...extra };
}

describe("mergeConsecutiveSegments", () => {
  it("merges close lines of one person only", () => {
    const merged = mergeConsecutiveSegments([
      seg("a", 0, { speaker_tag: "S1" }),
      seg("b", 1_000, { speaker_tag: "S1" }),
      seg("c", 2_000, { speaker_tag: "S2" }),
    ]);
    expect(merged.map((m) => m.originalIds)).toEqual([["a", "b"], ["c"]]);
  });

  it("leaves a line that is still growing on its own", () => {
    const merged = mergeConsecutiveSegments([seg("a", 0), seg("b", 1_000, { pending_text: "b and" })]);
    expect(merged.map((m) => m.originalIds)).toEqual([["a"], ["b"]]);
    expect(merged[1].pending_text).toBe("b and");
  });
});
