import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TranscriptRow, type TranscriptRowProps } from "./TranscriptRow";

function props(overrides: Partial<TranscriptRowProps> = {}): TranscriptRowProps {
  return {
    segment: {
      id: "s1",
      text: "We ship on Friday",
      speaker: "Them",
      speaker_id: "them",
      speaker_tag: "S2",
      timestamp_ms: 1,
      is_final: true,
      confidence: 1,
      mergedCount: 1,
      originalIds: ["s1"],
    },
    index: 0,
    isActive: false,
    isSelected: false,
    isBookmarked: false,
    searchQuery: "",
    isSearchMatch: false,
    timestampLabel: "0:01",
    speakerLabel: "Speaker 2",
    speakerColorClass: "",
    transcriptFontSize: 13,
    transcriptTextColor: "#fff",
    translationFontSize: 12,
    translationTextColor: "#fb0",
    canBookmark: false,
    onSeek: vi.fn(),
    onToggleBookmark: vi.fn(),
    ...overrides,
  };
}

describe("TranscriptRow remote speaker name", () => {
  it("renames the speaker on click + Enter without seeking", () => {
    const onRenameSpeaker = vi.fn();
    const onSeek = vi.fn();
    render(<TranscriptRow {...props({ renameKey: "them:S2", onRenameSpeaker, onSeek })} />);

    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByLabelText("Speaker name");
    fireEvent.change(input, { target: { value: "  Erin " } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onRenameSpeaker).toHaveBeenCalledWith("them:S2", "Erin");
    expect(onSeek).not.toHaveBeenCalled();
  });

  it("Escape keeps the name", () => {
    const onRenameSpeaker = vi.fn();
    render(<TranscriptRow {...props({ renameKey: "them:S2", onRenameSpeaker })} />);
    fireEvent.click(screen.getByRole("button", { name: "Speaker 2" }));
    const input = screen.getByLabelText("Speaker name");
    fireEvent.change(input, { target: { value: "Erin" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onRenameSpeaker).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Speaker 2" })).toBeTruthy();
  });

  it("other speakers are plain labels", () => {
    render(<TranscriptRow {...props()} />);
    expect(screen.queryByRole("button", { name: "Speaker 2" })).toBeNull();
    expect(screen.getByText("Speaker 2")).toBeTruthy();
  });
});
