import { describe, expect, it } from "vitest";
import { meetingEndRoute, meetingShortcutAction, shouldPersistOverlayData } from "./windows";

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

describe("meetingEndRoute", () => {
  it("the launcher ends through every window when the overlay took the meeting", () => {
    expect(meetingEndRoute("launcher", true)).toBe("all_windows");
  });

  it("the launcher ends by itself when no overlay took the meeting", () => {
    expect(meetingEndRoute("launcher", false)).toBe("here");
  });

  it("the overlay always ends by itself (it saves its own data)", () => {
    expect(meetingEndRoute("overlay", true)).toBe("here");
    expect(meetingEndRoute("overlay", false)).toBe("here");
  });

  it("agrees with shouldPersistOverlayData: a window that won't save the data asks the others", () => {
    for (const label of ["launcher", "overlay", ""]) {
      for (const overlayActive of [true, false]) {
        const savesHere = shouldPersistOverlayData(label, overlayActive);
        expect(meetingEndRoute(label, overlayActive) === "all_windows").toBe(!savesHere);
      }
    }
  });
});

describe("meetingShortcutAction", () => {
  it("ends a running meeting, even while a quit is pending", () => {
    expect(meetingShortcutAction(true, false)).toBe("end");
    expect(meetingShortcutAction(true, true)).toBe("end");
  });

  it("starts a meeting when none is running", () => {
    expect(meetingShortcutAction(false, false)).toBe("start");
  });

  it("never starts a meeting while a deferred quit is pending", () => {
    expect(meetingShortcutAction(false, true)).toBe("ignore");
  });
});
