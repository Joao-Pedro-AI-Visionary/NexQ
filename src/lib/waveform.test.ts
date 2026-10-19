import { describe, it, expect } from "vitest";
import { downsamplePeaks } from "./waveform";

describe("downsamplePeaks", () => {
  it("keeps the max per bucket", () => {
    expect(downsamplePeaks([0.1, 0.9, 0.2, 0.3], 2)).toEqual([0.9, 0.3]);
  });

  it("is a no-op when the input is already short enough", () => {
    expect(downsamplePeaks([0.1, 0.5, 0.2], 10)).toEqual([0.1, 0.5, 0.2]);
  });

  it("reduces a long input to exactly the target length", () => {
    expect(downsamplePeaks(new Array(87000).fill(0.5), 1200).length).toBe(1200);
  });

  it("returns an empty array for a non-positive target", () => {
    expect(downsamplePeaks([0.1, 0.2], 0)).toEqual([]);
  });
});
