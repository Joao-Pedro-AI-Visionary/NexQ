import "@testing-library/jest-dom/vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { LogEntry } from "../lib/types";
import { PromptViewer } from "./PromptViewer";

function entry(over: Partial<LogEntry> = {}): LogEntry {
  return {
    id: "e1",
    timestamp: 0,
    mode: "Assist",
    provider: "Claude (subscription)",
    model: "haiku",
    status: "complete",
    startedAt: 0,
    firstTokenAt: null,
    completedAt: null,
    totalTokens: 4389,
    latencyMs: 1200,
    usage: null,
    sessionKey: null,
    responseContent: "Answer",
    responseContentClean: "Answer",
    actualSystemPrompt: "s".repeat(4000),
    actualUserPrompt: "u".repeat(400),
    includeTranscript: true,
    includeRag: false,
    includeInstructions: false,
    includeQuestion: false,
    temperature: 0.3,
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

afterEach(cleanup);

describe("PromptViewer token row", () => {
  it("separates a session's one-time prompt from this request and shows the actual usage", () => {
    render(
      <PromptViewer
        entry={entry({
          sessionKey: "live:m1:fast",
          usage: {
            input_tokens: 10,
            cache_read_tokens: 4000,
            cache_write_tokens: 190,
            output_tokens: 189,
            thinking_tokens: 151,
            cost_usd: 0.0093,
            first_token_ms: 640,
          },
        })}
      />,
    );
    const row = screen.getByTestId("token-budget");
    expect(row).toHaveTextContent("Session prompt (sent once when the session started) ≈ 1,000");
    expect(row).toHaveTextContent("Sent this request ≈ 100");
    expect(screen.getByTestId("token-actual")).toHaveTextContent(
      "Actual: 4,200 in (4,000 cached · 190 written) · 189 out (151 thinking) · ≈$0.0093",
    );
  });

  it("labels a stateless request's numbers as an estimate", () => {
    render(<PromptViewer entry={entry({ provider: "OpenAI" })} />);
    expect(screen.getByTestId("token-budget")).toHaveTextContent("≈ estimate");
    expect(screen.queryByTestId("token-actual")).toBeNull();
  });
});
