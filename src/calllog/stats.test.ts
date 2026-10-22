import { describe, expect, it } from "vitest";
import type { LogEntry, StreamUsage } from "../lib/types";
import { callLogStats, entryFirstTokenMs, entryTokens, formatActualUsage, isClaudeEntry } from "./stats";

function entry(over: Partial<LogEntry> = {}): LogEntry {
  return {
    id: Math.random().toString(36),
    timestamp: 0,
    mode: "Assist",
    provider: "OpenAI",
    model: "gpt-4o",
    status: "complete",
    startedAt: 1000,
    firstTokenAt: null,
    completedAt: null,
    totalTokens: null,
    latencyMs: null,
    usage: null,
    sessionKey: null,
    responseContent: "",
    responseContentClean: "",
    actualSystemPrompt: "",
    actualUserPrompt: "",
    includeTranscript: false,
    includeRag: false,
    includeInstructions: false,
    includeQuestion: false,
    temperature: null,
    ragQuery: null,
    ragChunks: [],
    ragChunksFiltered: 0,
    ragTotalCandidates: 0,
    transcriptWindowSeconds: null,
    transcriptSegmentsCount: null,
    transcriptSegmentsTotal: null,
    snapshotTranscript: "",
    snapshotContext: "",
    reconstructedSystemPrompt: "",
    errorMessage: null,
    ...over,
  };
}

const usage = (over: Partial<StreamUsage> = {}): StreamUsage => ({
  input_tokens: 10,
  cache_read_tokens: 4000,
  cache_write_tokens: 190,
  output_tokens: 189,
  thinking_tokens: 151,
  cost_usd: 0.0093,
  first_token_ms: 640,
  ...over,
});

describe("call log stats", () => {
  it("averages completed answers, counts actual tokens from usage and every error", () => {
    const stats = callLogStats([
      entry({ latencyMs: 1200, usage: usage() }),
      entry({ latencyMs: 1800, firstTokenAt: 1900, totalTokens: 50 }),
      entry({ status: "error", latencyMs: null }),
      entry({ status: "streaming", firstTokenAt: 1100 }),
      entry({ status: "cancelled", latencyMs: 400 }),
    ]);
    expect(stats.total).toBe(5);
    expect(stats.avgLatencyMs).toBe(1500);
    // 640 ms reported by the backend, 900 ms measured from the timestamps.
    expect(stats.avgFirstTokenMs).toBe(770);
    expect(stats.totalTokens).toBe(10 + 4000 + 190 + 189 + 50);
    expect(stats.errorCount).toBe(1);
  });

  it("has no averages before anything completed", () => {
    const stats = callLogStats([entry({ status: "sending" })]);
    expect(stats.avgLatencyMs).toBeNull();
    expect(stats.avgFirstTokenMs).toBeNull();
    expect(stats.totalTokens).toBe(0);
  });

  it("prefers the backend's first-token time over the frontend's", () => {
    expect(entryFirstTokenMs(entry({ firstTokenAt: 3000, usage: usage({ first_token_ms: 500 }) }))).toBe(500);
    expect(entryFirstTokenMs(entry({ firstTokenAt: 3000 }))).toBe(2000);
    expect(entryFirstTokenMs(entry())).toBeNull();
    expect(entryTokens(entry({ totalTokens: 7 }))).toBe(7);
  });

  it("recognises Claude entries by session or provider name", () => {
    expect(isClaudeEntry(entry({ provider: "Claude (subscription)" }))).toBe(true);
    expect(isClaudeEntry(entry({ sessionKey: "live:m1:fast" }))).toBe(true);
    expect(isClaudeEntry(entry({ provider: "Anthropic" }))).toBe(false);
  });

  it("formats the actual usage line", () => {
    expect(formatActualUsage(usage())).toBe("4,200 in (4,000 cached · 190 written) · 189 out (151 thinking) · ≈$0.0093");
    expect(formatActualUsage(usage({ cache_read_tokens: 0, cache_write_tokens: 0, thinking_tokens: 0, cost_usd: null }))).toBe(
      "10 in · 189 out",
    );
  });
});
