import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, vi } from "vitest";
import { MeetingCard } from "./MeetingCard";
import type { MeetingSummary } from "../lib/types";

const TITLE = "Meeting 2026-10-19 13:00";

function makeMeeting(title = TITLE): MeetingSummary {
  return {
    id: "m-1",
    title,
    start_time: "2026-10-19T13:00:00Z",
    end_time: "2026-10-19T13:30:00Z",
    duration_seconds: 1800,
    segment_count: 4,
    has_summary: false,
  };
}

function setup() {
  const onRename = vi.fn().mockResolvedValue(undefined);
  const user = userEvent.setup();
  render(
    <MeetingCard
      meeting={makeMeeting()}
      onSelect={vi.fn()}
      onDelete={vi.fn()}
      onRename={onRename}
    />
  );
  return { onRename, user };
}

describe("MeetingCard rename", () => {
  it("clicking save commits the edited title once", async () => {
    const { onRename, user } = setup();
    await user.click(screen.getByRole("button", { name: /rename/i }));
    const input = screen.getByRole("textbox");
    await user.clear(input);
    await user.type(input, "Team sync");
    await user.click(screen.getByRole("button", { name: /save/i }));
    expect(onRename).toHaveBeenCalledTimes(1);
    expect(onRename).toHaveBeenCalledWith("m-1", "Team sync");
  });

  it("clicking cancel discards the edit without saving", async () => {
    const { onRename, user } = setup();
    await user.click(screen.getByRole("button", { name: /rename/i }));
    const input = screen.getByRole("textbox");
    await user.clear(input);
    await user.type(input, "Team sync");
    await user.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.getByText(TITLE)).toBeInTheDocument();
  });

  it("opens the editor with the whole title selected from the start", async () => {
    const setSelectionRange = vi.spyOn(HTMLInputElement.prototype, "setSelectionRange");
    const { user } = setup();
    await user.click(screen.getByRole("button", { name: /rename/i }));
    const input = screen.getByRole("textbox") as HTMLInputElement;
    expect(setSelectionRange).toHaveBeenCalledWith(0, TITLE.length);
    expect(input.selectionStart).toBe(0);
    expect(input.selectionEnd).toBe(TITLE.length);
    expect(input.scrollLeft).toBe(0);
    setSelectionRange.mockRestore();
  });
});
