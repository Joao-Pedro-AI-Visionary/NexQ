import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../lib/events", () => ({ onQuestionDetected: () => Promise.resolve(() => {}), onStreamEnd: () => Promise.resolve(() => {}) }));
vi.mock("../lib/logger", () => ({ logToFile: vi.fn() }));
vi.mock("../lib/userAction", () => ({ runUserAction: vi.fn(() => Promise.resolve()) }));

import { QuestionDetector, lastSentence, looksLikeQuestion } from "./QuestionDetector";
import { runUserAction } from "../lib/userAction";
import { useTranscriptStore } from "../stores/transcriptStore";
import type { TranscriptSegment } from "../lib/types";

const seg = (text: string): TranscriptSegment =>
  ({ id: "them_1", text, speaker: "Them", timestamp_ms: 1, is_final: true, confidence: 1 }) as TranscriptSegment;

describe("QuestionDetector", () => {
  it("checks the newest sentence of a line", () => {
    expect(lastSentence("So here's the situation. What would you do?")).toBe("What would you do?");
    expect(looksLikeQuestion(lastSentence("We run preview days. How many do you run"))).toBe(true);
  });

  it("finds a question a line gains after its first final", () => {
    useTranscriptStore.setState({ segments: [seg("So here's the situation.")] });
    render(<QuestionDetector />);
    expect(screen.queryByText(/What would you do/)).toBeNull();

    act(() => useTranscriptStore.setState({ segments: [seg("So here's the situation. What would you do?")] }));
    expect(screen.getByText(/What would you do\?/)).toBeTruthy();
  });

  it("detects pt-BR questions", () => {
    expect(looksLikeQuestion("Me conta um pouco da sua experiência com React")).toBe(true);
    expect(looksLikeQuestion("Qual foi o maior desafio do seu último projeto")).toBe(true);
    expect(looksLikeQuestion("Hoje está chovendo bastante.")).toBe(false);
  });

  it("runs Assist by itself when the other party asks a question", () => {
    vi.useFakeTimers();
    useTranscriptStore.setState({ segments: [] });
    render(<QuestionDetector />);
    act(() => useTranscriptStore.setState({ segments: [{ ...seg("Por que você quer trabalhar aqui?"), id: "them_9" }] }));
    act(() => { vi.advanceTimersByTime(1500); });
    expect(runUserAction).toHaveBeenCalledWith("Assist", "Por que você quer trabalhar aqui?");
    vi.useRealTimers();
  });
});
