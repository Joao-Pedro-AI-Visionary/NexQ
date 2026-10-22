import { describe, expect, it } from "vitest";
import {
  EMPTY_USAGE,
  avgFirstTokenMs,
  avgLatencyMs,
  formatAgo,
  formatDuration,
  formatTokenUsage,
  formatUsageSummary,
  lastAnswerTooltip,
  promptTokens,
} from "./agentUsage";

describe("formatDuration", () => {
  it.each([
    [0, "0 ms"],
    [850, "850 ms"],
    [999, "999 ms"],
    [1000, "1.0 s"],
    [1234, "1.2 s"],
    [9_960, "10.0 s"],
    [30_189, "30 s"],
    [125_000, "2 min 5 s"],
    [120_000, "2 min"],
  ])("%d ms → %s", (ms, text) => {
    expect(formatDuration(ms)).toBe(text);
  });

  it("guards against nonsense", () => {
    expect(formatDuration(-1)).toBe("—");
    expect(formatDuration(Number.NaN)).toBe("—");
  });
});

describe("formatAgo", () => {
  it("is relative and coarse", () => {
    const now = 1_000_000;
    expect(formatAgo(now - 3_000, now)).toBe("just now");
    expect(formatAgo(now - 40_000, now)).toBe("40 s ago");
    expect(formatAgo(now - 125_000, now)).toBe("2 min ago");
    expect(formatAgo(now - 3 * 3_600_000, now)).toBe("3 h ago");
  });
});

describe("usage formatting", () => {
  const usage = {
    ...EMPTY_USAGE,
    input_tokens: 210,
    cache_read_tokens: 38_000,
    cache_write_tokens: 7_000,
    output_tokens: 1_240,
    thinking_tokens: 300,
    cost_usd: 0.31,
    requests: 3,
    total_latency_ms: 4_200,
    total_first_token_ms: 1_800,
  };

  it("counts cache reads and writes as prompt tokens", () => {
    expect(promptTokens(usage)).toBe(45_210);
    expect(formatTokenUsage(usage)).toBe("in 45,210 (38,000 cached) · out 1,240 (thinking 300)");
    expect(formatTokenUsage({ ...EMPTY_USAGE, input_tokens: 5, output_tokens: 2 })).toBe("in 5 · out 2");
  });

  it("summarises requests, tokens and the average response time", () => {
    expect(formatUsageSummary(usage)).toBe("3 requests · 45,210 in (38,000 cached) / 1,240 out · avg 1.4 s");
    expect(formatUsageSummary({ ...EMPTY_USAGE, requests: 1, input_tokens: 9, output_tokens: 1, total_latency_ms: 800 })).toBe(
      "1 request · 9 in / 1 out · avg 800 ms",
    );
    expect(formatUsageSummary(EMPTY_USAGE)).toBe("0 requests · 0 in / 0 out");
  });

  it("averages per answered request", () => {
    expect(avgLatencyMs(usage)).toBe(1_400);
    expect(avgFirstTokenMs(usage)).toBe(600);
    expect(avgLatencyMs(EMPTY_USAGE)).toBeNull();
  });

  it("describes the last answer for the footer tooltip", () => {
    expect(lastAnswerTooltip(1_200, 600)).toBe("Last answer: 1.2 s total, first words after 600 ms");
    expect(lastAnswerTooltip(31_000, null)).toBe("Last answer: 31 s total");
  });
});
