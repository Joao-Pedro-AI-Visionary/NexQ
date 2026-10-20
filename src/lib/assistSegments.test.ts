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

  it("does not mutate the input", () => {
    assistSegments(finals, true);
    expect(finals[0].text).toBe("Hi");
  });
});
