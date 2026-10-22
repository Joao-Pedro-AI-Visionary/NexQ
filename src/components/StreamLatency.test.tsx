import "@testing-library/jest-dom/vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (payload: unknown) => void;
const { handlers, register } = vi.hoisted(() => {
  const handlers: Record<string, Handler> = {};
  const register = (name: string) => (handler: Handler) => {
    handlers[name] = handler;
    return Promise.resolve(() => {
      delete handlers[name];
    });
  };
  return { handlers, register };
});

vi.mock("../lib/events", () => ({
  onStreamStart: register("start"),
  onStreamToken: register("token"),
  onStreamThinking: register("thinking"),
  onStreamEnd: register("end"),
  onStreamError: register("error"),
  onStreamSources: register("sources"),
}));

import { StreamLatency } from "./StreamLatency";
import { useStreamStore } from "../stores/streamStore";
import { resetStreamTimingForTests } from "../stores/streamTimingStore";

describe("StreamLatency (footer)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetStreamTimingForTests();
    useStreamStore.setState({ isStreaming: false, latencyMs: null });
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("shows nothing before the first answer", () => {
    render(<StreamLatency />);
    expect(screen.queryByTestId("stream-latency")).toBeNull();
  });

  it("runs a live Thinking → Writing timer, then shows the last answer with its first-word time", async () => {
    render(<StreamLatency />);
    await act(async () => {});
    act(() => {
      handlers.start({ mode: "Assist", request_id: "r1" });
      useStreamStore.setState({ isStreaming: true, latencyMs: null });
    });
    act(() => vi.advanceTimersByTime(2_300));
    expect(screen.getByTestId("stream-latency")).toHaveTextContent("Thinking 2.3 s");

    act(() => {
      handlers.token({ token: "Hi", request_id: "r1" });
      vi.advanceTimersByTime(800);
    });
    expect(screen.getByTestId("stream-latency")).toHaveTextContent("Writing 3.1 s");

    act(() => {
      handlers.end({
        total_tokens: 1,
        latency_ms: 31_189,
        request_id: "r1",
        cancelled: false,
        usage: { input_tokens: 1, cache_read_tokens: 0, cache_write_tokens: 0, output_tokens: 1, thinking_tokens: 0, cost_usd: null, first_token_ms: 640 },
      });
      useStreamStore.setState({ isStreaming: false, latencyMs: 31_189 });
    });
    const el = screen.getByTestId("stream-latency");
    expect(el).toHaveTextContent("31 s");
    expect(el).toHaveAttribute("title", "Last answer: 31 s total, first words after 640 ms");
  });

  it("formats a quick answer in ms and ignores past-meeting streams", async () => {
    render(<StreamLatency />);
    await act(async () => {});
    act(() => {
      handlers.start({ mode: "MeetingSummary", request_id: "past" });
      useStreamStore.setState({ isStreaming: false, latencyMs: 850 });
    });
    expect(screen.getByTestId("stream-latency")).toHaveTextContent("850 ms");
  });
});
