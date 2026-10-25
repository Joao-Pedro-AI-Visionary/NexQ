import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(() => Promise.resolve());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

describe("logToFile", () => {
  beforeEach(() => {
    vi.resetModules();
    invoke.mockClear();
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 9, 24, 10, 0));
  });

  const sent = () => invoke.mock.calls.map((c) => (c as unknown[])[1] as { level: string; message: string });

  it("writes a line, collapsing repeats", async () => {
    const { logToFile } = await import("./logger");
    logToFile("error", "overlay", "STT failed");
    logToFile("error", "overlay", "STT failed");
    logToFile("error", "overlay", "STT failed");
    logToFile("warn", "overlay", "next");
    expect(sent().map((c) => c.message)).toEqual([
      "STT failed",
      "(previous message repeated 2 more times)",
      "next",
    ]);
  });

  it("caps lines per minute and reports what it skipped", async () => {
    const { logToFile } = await import("./logger");
    for (let i = 0; i < 70; i++) logToFile("warn", "launcher", `line ${i}`);
    expect(invoke).toHaveBeenCalledTimes(60);
    vi.advanceTimersByTime(61_000);
    logToFile("warn", "launcher", "after");
    expect(sent().slice(-2).map((c) => c.message)).toEqual(["10 log lines skipped (rate limit)", "after"]);
  });
});
