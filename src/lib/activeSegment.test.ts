import { describe, it, expect } from "vitest";
import { findActiveSegmentIndex } from "./activeSegment";

describe("findActiveSegmentIndex", () => {
  const starts = [0, 3000, 6000];

  it("returns the last start at or before the time", () => {
    expect(findActiveSegmentIndex(starts, 0)).toBe(0);
    expect(findActiveSegmentIndex(starts, 2999)).toBe(0);
    expect(findActiveSegmentIndex(starts, 3000)).toBe(1);
    expect(findActiveSegmentIndex(starts, 99999)).toBe(2);
  });

  it("returns -1 before the first start", () => {
    expect(findActiveSegmentIndex([500], 100)).toBe(-1);
  });

  it("returns -1 for empty starts", () => {
    expect(findActiveSegmentIndex([], 1000)).toBe(-1);
  });
});
