import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { generateAssist } from "./ipc";
import { useTranscriptStore } from "../stores/transcriptStore";

describe("generateAssist", () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(undefined);
  });

  it("sends a fresh request id and the live transcript by default", async () => {
    useTranscriptStore.setState({
      segments: [
        { id: "a", text: "hello", speaker: "Them", timestamp_ms: 1, is_final: true },
        { id: "b", text: "partial", speaker: "Them", timestamp_ms: 2, is_final: false },
      ],
    } as never);

    await generateAssist("Assist");
    await generateAssist("Assist");

    const first = invoke.mock.calls[0][1];
    const second = invoke.mock.calls[1][1];
    expect(invoke.mock.calls[0][0]).toBe("generate_assist");
    expect(typeof first.requestId).toBe("string");
    expect(first.requestId).not.toBe(second.requestId);
    expect(JSON.parse(first.transcriptSegments)).toEqual([
      { id: "a", text: "hello", speaker: "Them", timestamp_ms: 1 },
    ]);
  });

  it("uses the caller's request id and transcript segments", async () => {
    const segments = [{ id: "s1", text: "past", speaker: "You", timestamp_ms: 5 }];

    await generateAssist("BookmarkSuggestions", "Find moments", {
      requestId: "req-42",
      transcriptSegments: segments,
    });

    expect(invoke).toHaveBeenCalledWith("generate_assist", {
      mode: "BookmarkSuggestions",
      customQuestion: "Find moments",
      transcriptSegments: JSON.stringify(segments),
      requestId: "req-42",
    });
  });

  it("sends a picked scope, and nothing for the default", async () => {
    const scope = { kind: "minutes" as const, since_ms: 100, minutes: 5, label: "Last 5 min" };
    await generateAssist("Recap", undefined, { requestId: "r", scope });
    expect(invoke.mock.calls[0][1].scope).toEqual(scope);

    await generateAssist("Recap", undefined, { scope: { kind: "default", since_ms: null, label: "" } });
    await generateAssist("Recap", undefined, { scope: null });
    expect("scope" in invoke.mock.calls[1][1]).toBe(false);
    expect("scope" in invoke.mock.calls[2][1]).toBe(false);
  });
});
