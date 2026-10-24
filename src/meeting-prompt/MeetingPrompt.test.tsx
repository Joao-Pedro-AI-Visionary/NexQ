import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MeetingPrompt } from "./MeetingPrompt";
import type { MeetingPromptInfo } from "../lib/types";

const info = (over: Partial<MeetingPromptInfo["meeting"]> = {}, secs = 60): MeetingPromptInfo => ({
  meeting: { app: "zoom", app_name: "Zoom", title: null, ...over },
  auto_dismiss_secs: secs,
  default_scenario: "interview",
});

describe("MeetingPrompt", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2026, 9, 23, 15, 2));
  });
  afterEach(() => vi.useRealTimers());

  it("starts a meeting named after the call, with the default scenario", () => {
    const onStart = vi.fn();
    render(<MeetingPrompt info={info({ title: "Weekly sync" })} onStart={onStart} onDismiss={() => {}} />);
    expect(screen.getByText("Zoom meeting detected")).toBeInTheDocument();
    expect(screen.getByLabelText("Meeting name")).toHaveValue("Weekly sync");
    fireEvent.click(screen.getByRole("button", { name: /Start NexQ/ }));
    expect(onStart).toHaveBeenCalledWith("Weekly sync — Meeting 2026-10-23 15:02", "interview");
  });

  it("uses the app's name without a title, and an edited name and type when given", () => {
    const onStart = vi.fn();
    render(<MeetingPrompt info={info()} onStart={onStart} onDismiss={() => {}} />);
    fireEvent.keyDown(window, { key: "Enter" });
    expect(onStart).toHaveBeenLastCalledWith("Zoom — Meeting 2026-10-23 15:02", "interview");

    fireEvent.change(screen.getByLabelText("Meeting name"), { target: { value: "Board review" } });
    fireEvent.change(screen.getByLabelText(/Meeting type/), { target: { value: "team_meeting" } });
    fireEvent.click(screen.getByRole("button", { name: /Start NexQ/ }));
    expect(onStart).toHaveBeenLastCalledWith("Board review — Meeting 2026-10-23 15:02", "team_meeting");
  });

  it("goes away after the timeout, but not while hovered; Escape dismisses", () => {
    const onDismiss = vi.fn();
    render(<MeetingPrompt info={info({}, 2)} onStart={() => {}} onDismiss={onDismiss} />);
    const card = screen.getByRole("dialog");
    fireEvent.mouseEnter(card);
    act(() => vi.advanceTimersByTime(3000));
    expect(onDismiss).not.toHaveBeenCalled();
    fireEvent.mouseLeave(card);
    act(() => vi.advanceTimersByTime(2100));
    expect(onDismiss).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDismiss).toHaveBeenCalledTimes(2);
  });

  it("stays until the call ends when the timeout is off", () => {
    const onDismiss = vi.fn();
    render(<MeetingPrompt info={info({}, 0)} onStart={() => {}} onDismiss={onDismiss} />);
    act(() => vi.advanceTimersByTime(600_000));
    expect(onDismiss).not.toHaveBeenCalled();
  });
});
