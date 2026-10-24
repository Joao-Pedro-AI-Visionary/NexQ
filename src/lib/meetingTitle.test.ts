import { describe, expect, it } from "vitest";
import { composeMeetingTitle } from "./meetingTitle";

const at = new Date(2026, 9, 23, 15, 2);

describe("composeMeetingTitle", () => {
  it("puts the call's title in front of the usual name", () => {
    expect(composeMeetingTitle("Weekly sync", at)).toBe("Weekly sync — Meeting 2026-10-23 15:02");
    expect(composeMeetingTitle("  Q4   planning ", at)).toBe("Q4 planning — Meeting 2026-10-23 15:02");
  });

  it("falls back to the usual name without a title", () => {
    expect(composeMeetingTitle("", at)).toBe("Meeting 2026-10-23 15:02");
    expect(composeMeetingTitle(null, at)).toBe("Meeting 2026-10-23 15:02");
  });
});
