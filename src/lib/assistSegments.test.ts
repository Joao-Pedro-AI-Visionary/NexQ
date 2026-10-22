import { describe, expect, it } from "vitest";
import { assistSegments } from "./assistSegments";

const finals = [
  { id: "a", text: "Hi", speaker: "Them", timestamp_ms: 1 },
  { id: "b", text: "there", speaker: "Them", timestamp_ms: 2 },
  { id: "c", text: "Hello", speaker: "User", timestamp_ms: 3 },
  { id: "d", text: "again", speaker: "Them", timestamp_ms: 4 },
];

describe("assistSegments", () => {
  it("sends every segment once with its own id and text for a Claude session", () => {
    const out = assistSegments(finals, false);
    expect(out.map((s) => s.id)).toEqual(["a", "b", "c", "d"]);
    // No text travels under another segment's id (no duplicate, no drop).
    for (const seg of out) {
      expect(seg.text).toBe(finals.find((f) => f.id === seg.id)!.text);
    }
  });

  it("merges same-speaker runs for the stateless path", () => {
    expect(assistSegments(finals, true)).toEqual([
      { id: "a", text: "Hi there", speaker: "Them", timestamp_ms: 1 },
      { id: "c", text: "Hello", speaker: "User", timestamp_ms: 3 },
      { id: "d", text: "again", speaker: "Them", timestamp_ms: 4 },
    ]);
  });

  it("labels diarized lines with the meeting's speaker names and keeps people apart", () => {
    const tagged = [
      { id: "a", text: "We ship", speaker: "Them", speaker_id: "them", speaker_tag: "S1", timestamp_ms: 1 },
      { id: "b", text: "Friday", speaker: "Them", speaker_id: "them", speaker_tag: "S1", timestamp_ms: 2 },
      { id: "c", text: "No", speaker: "Them", speaker_id: "them", speaker_tag: "S2", timestamp_ms: 3 },
    ];
    const speakers = [
      { id: "them:S1", display_name: "Erin", source: "remote_tag" as const, stats: { segment_count: 0, word_count: 0, talk_time_ms: 0, last_spoke_ms: 0 } },
    ];
    expect(assistSegments(tagged, true, speakers)).toEqual([
      { id: "a", text: "We ship Friday", speaker: "Erin", timestamp_ms: 1 },
      { id: "c", text: "No", speaker: "Speaker 2", timestamp_ms: 3 },
    ]);
    expect(assistSegments(tagged, false).map((s) => s.speaker)).toEqual(["Speaker 1", "Speaker 1", "Speaker 2"]);
  });

  it("does not mutate the input", () => {
    assistSegments(finals, true);
    expect(finals[0].text).toBe("Hi");
  });
});
