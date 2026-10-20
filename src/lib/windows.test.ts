import { describe, expect, it } from "vitest";
import { shouldPersistOverlayData } from "./windows";

describe("shouldPersistOverlayData", () => {
  it("the overlay window persists overlay-edited data", () => {
    expect(shouldPersistOverlayData("overlay", true)).toBe(true);
  });

  it("the launcher defers to an overlay that took the meeting", () => {
    expect(shouldPersistOverlayData("launcher", true)).toBe(false);
  });

  it("the launcher persists when no overlay took the meeting", () => {
    expect(shouldPersistOverlayData("launcher", false)).toBe(true);
  });

  it("outside Tauri (no label) behaves like the launcher", () => {
    expect(shouldPersistOverlayData("", false)).toBe(true);
    expect(shouldPersistOverlayData("", true)).toBe(false);
  });
});
