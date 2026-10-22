import { beforeEach, describe, expect, it } from "vitest";
import { useTranscriptStore } from "./transcriptStore";
import type { TranscriptSegment } from "../lib/types";

function seg(text: string, is_final: boolean, id = "them_1_acc_1"): TranscriptSegment {
  return { id, text, speaker: "Them", speaker_id: "them", timestamp_ms: 1_000, is_final, confidence: 0.9 };
}

describe("transcriptStore upsert", () => {
  beforeEach(() => useTranscriptStore.getState().clearSegments());

  it("replaces a line in place when its final grows (same id)", () => {
    const { updateInterimSegment } = useTranscriptStore.getState();
    updateInterimSegment(seg("We ship", true));
    updateInterimSegment(seg("other", true, "you_1_acc_1"));
    updateInterimSegment(seg("We ship on Friday", true));
    const { segments } = useTranscriptStore.getState();
    expect(segments.map((s) => s.text)).toEqual(["We ship on Friday", "other"]);
  });

  it("keeps a final line final while interim words extend it", () => {
    const { updateInterimSegment } = useTranscriptStore.getState();
    updateInterimSegment(seg("We ship", true));
    updateInterimSegment(seg("We ship on fri", false));
    let line = useTranscriptStore.getState().segments[0];
    expect(line).toMatchObject({ text: "We ship", is_final: true, pending_text: "We ship on fri" });

    updateInterimSegment(seg("We ship on Friday.", true));
    line = useTranscriptStore.getState().segments[0];
    expect(line).toMatchObject({ text: "We ship on Friday.", is_final: true });
    expect(line.pending_text).toBeUndefined();
  });

  it("an interim for a new line is added as interim", () => {
    useTranscriptStore.getState().updateInterimSegment(seg("hel", false));
    expect(useTranscriptStore.getState().segments[0]).toMatchObject({ text: "hel", is_final: false });
  });

  it("finalizing interims commits pending words", () => {
    const { updateInterimSegment, finalizeAllInterim } = useTranscriptStore.getState();
    updateInterimSegment(seg("We ship", true));
    updateInterimSegment(seg("We ship today", false));
    updateInterimSegment(seg("Sure", false, "you_1_acc_1"));
    finalizeAllInterim();
    const { segments } = useTranscriptStore.getState();
    expect(segments.map((s) => [s.text, s.is_final, s.pending_text])).toEqual([
      ["We ship today", true, undefined],
      ["Sure", true, undefined],
    ]);
  });
});
