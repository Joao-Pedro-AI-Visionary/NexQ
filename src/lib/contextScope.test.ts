import { describe, expect, it } from "vitest";
import {
  clampMinutes,
  resolveScope,
  scopeChipLabel,
  scopeHint,
  type LastUserRequests,
} from "./contextScope";

const NOW = 1_700_000_900_000;
const none: LastUserRequests = { anyAt: null, byMode: {} };
const some: LastUserRequests = { anyAt: NOW - 60_000, byMode: { FollowUp: NOW - 300_000 } };

describe("resolveScope", () => {
  it("default sends no scope (the backend keeps today's behaviour)", () => {
    expect(resolveScope({ kind: "default", minutes: 5 }, "FollowUp", "F/U", some, NOW)).toBeNull();
  });

  it("whole meeting", () => {
    expect(resolveScope({ kind: "all", minutes: 5 }, "Recap", "Recap", some, NOW)).toEqual({
      kind: "all",
      since_ms: null,
      label: "Whole meeting",
    });
  });

  it("last N minutes counts back from now", () => {
    expect(resolveScope({ kind: "minutes", minutes: 10 }, "Recap", "Recap", none, NOW)).toEqual({
      kind: "minutes",
      since_ms: NOW - 600_000,
      minutes: 10,
      label: "Last 10 min",
    });
  });

  it("since the last answer of any action", () => {
    expect(resolveScope({ kind: "since_last_any", minutes: 5 }, "Assist", "Assist", some, NOW)).toEqual({
      kind: "since_last_any",
      since_ms: NOW - 60_000,
      label: "Since last answer",
    });
  });

  it("since the last answer of the same action", () => {
    expect(resolveScope({ kind: "since_last_same", minutes: 5 }, "FollowUp", "F/U", some, NOW)).toEqual({
      kind: "since_last_same",
      since_ms: NOW - 300_000,
      label: "Since last F/U",
    });
  });

  it("no previous request behaves like the whole meeting, and says why", () => {
    expect(resolveScope({ kind: "since_last_same", minutes: 5 }, "Shorten", "Short", some, NOW)).toEqual({
      kind: "all",
      since_ms: null,
      label: "Whole meeting (first Short)",
    });
    expect(resolveScope({ kind: "since_last_any", minutes: 5 }, "Assist", "Assist", none, NOW)).toEqual({
      kind: "all",
      since_ms: null,
      label: "Whole meeting (first answer)",
    });
  });

  it("clamps out-of-range minutes", () => {
    expect(resolveScope({ kind: "minutes", minutes: 500 }, "Recap", "Recap", none, NOW)?.minutes).toBe(120);
    expect(clampMinutes(0)).toBe(1);
    expect(clampMinutes(7.6)).toBe(8);
    expect(clampMinutes(Number.NaN)).toBe(5);
  });
});

describe("labels", () => {
  it("chip labels", () => {
    expect(scopeChipLabel({ kind: "default", minutes: 5 })).toBe("Action default");
    expect(scopeChipLabel({ kind: "since_last_any", minutes: 5 })).toBe("Since last answer");
    expect(scopeChipLabel({ kind: "since_last_same", minutes: 5 })).toBe("Since last same");
    expect(scopeChipLabel({ kind: "minutes", minutes: 5 })).toBe("Last 5 min");
    expect(scopeChipLabel({ kind: "all", minutes: 5 })).toBe("Whole meeting");
  });

  it("action hints mention the effective scope", () => {
    expect(scopeHint({ kind: "minutes", minutes: 5 }, "F/U")).toBe("last 5 min");
    expect(scopeHint({ kind: "since_last_same", minutes: 5 }, "F/U")).toBe("since last F/U");
    expect(scopeHint({ kind: "default", minutes: 5 }, "F/U")).toBeNull();
  });
});
