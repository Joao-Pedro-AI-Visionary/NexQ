import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-store", () => ({
  load: async () => ({ get: async () => undefined, set: async () => {}, onKeyChange: async () => () => {} }),
}));

import { ContextScopePicker } from "./ContextScopePicker";
import { useConfigStore } from "../stores/configStore";

const scope = () => useConfigStore.getState().contextScope;
const chip = () => screen.getByRole("button", { name: /Transcript scope/ });
const openPicker = () => fireEvent.click(chip());

describe("ContextScopePicker", () => {
  beforeEach(() => {
    useConfigStore.setState({ contextScope: { kind: "default", minutes: 5 }, contextScopeSticky: true });
  });

  it("shows the current scope on the chip, calm by default and highlighted otherwise", () => {
    const { rerender } = render(<ContextScopePicker />);
    expect(chip()).toHaveTextContent("Action default");
    expect(chip().className).not.toContain("bg-primary");
    act(() => useConfigStore.setState({ contextScope: { kind: "minutes", minutes: 10 } }));
    rerender(<ContextScopePicker />);
    expect(chip()).toHaveTextContent("Last 10 min");
    expect(chip().className).toContain("bg-primary/10");
  });

  it("opens on the selected option; arrows move focus, Enter selects and closes", () => {
    useConfigStore.setState({ contextScope: { kind: "since_last_any", minutes: 5 } });
    render(<ContextScopePicker />);
    openPicker();
    expect(screen.getByRole("dialog", { name: "Transcript scope" })).toBeInTheDocument();
    const radios = screen.getAllByRole("radio");
    expect(radios).toHaveLength(5);
    expect(document.activeElement).toBe(radios[1]);

    fireEvent.keyDown(radios[1], { key: "ArrowDown" });
    expect(document.activeElement).toBe(radios[2]);
    fireEvent.keyDown(radios[2], { key: "ArrowUp" });
    fireEvent.keyDown(radios[1], { key: "ArrowUp" });
    fireEvent.keyDown(radios[0], { key: "ArrowUp" });
    expect(document.activeElement).toBe(radios[4]); // wraps
    fireEvent.keyDown(radios[4], { key: "Enter" });

    expect(scope()).toEqual({ kind: "all", minutes: 5 });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(chip());
  });

  it("left/right on the minutes option steps through the presets", () => {
    render(<ContextScopePicker />);
    openPicker();
    const minutes = screen.getAllByRole("radio")[3];
    fireEvent.keyDown(minutes, { key: "ArrowRight" });
    expect(scope()).toEqual({ kind: "minutes", minutes: 10 });
    fireEvent.keyDown(minutes, { key: "ArrowLeft" });
    fireEvent.keyDown(minutes, { key: "ArrowLeft" });
    expect(scope()).toEqual({ kind: "minutes", minutes: 2 });
    expect(screen.getByRole("dialog")).toBeInTheDocument(); // stays open while stepping
  });

  it("presets and the custom input pick minutes", () => {
    render(<ContextScopePicker />);
    openPicker();
    fireEvent.click(screen.getByRole("button", { name: "Last 15 minutes" }));
    expect(scope()).toEqual({ kind: "minutes", minutes: 15 });

    openPicker();
    const input = screen.getByRole("spinbutton", { name: /Custom minutes/ });
    fireEvent.change(input, { target: { value: "45" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(scope()).toEqual({ kind: "minutes", minutes: 45 });
    expect(chip()).toHaveTextContent("Last 45 min");

    openPicker();
    const again = screen.getByRole("spinbutton", { name: /Custom minutes/ });
    expect(again).toHaveValue(45);
    fireEvent.change(again, { target: { value: "999" } });
    fireEvent.keyDown(again, { key: "Enter" });
    expect(scope().minutes).toBe(120);
  });

  it("Escape closes and refocuses the chip; a click outside closes", () => {
    render(<ContextScopePicker />);
    openPicker();
    fireEvent.keyDown(screen.getAllByRole("radio")[0], { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(chip());

    openPicker();
    fireEvent.mouseDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keys inside the popover don't reach the window action shortcuts", () => {
    const onWindowKey = vi.fn();
    window.addEventListener("keydown", onWindowKey);
    render(<ContextScopePicker />);
    openPicker();
    fireEvent.keyDown(screen.getAllByRole("radio")[0], { key: " ", code: "Space" });
    window.removeEventListener("keydown", onWindowKey);
    expect(onWindowKey).not.toHaveBeenCalled();
  });

  it("the switch keeps or releases the scope after one action", () => {
    useConfigStore.setState({ contextScope: { kind: "all", minutes: 5 } });
    render(<ContextScopePicker />);
    openPicker();
    const sw = screen.getByRole("switch", { name: "Keep for next actions" });
    expect(sw).toHaveAttribute("aria-checked", "true");
    fireEvent.click(sw);
    expect(useConfigStore.getState().contextScopeSticky).toBe(false);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(chip()).toHaveAccessibleName(/next action only/);
  });
});

describe("ContextScopePicker: what Action default means", () => {
  beforeEach(() => {
    useConfigStore.setState({ contextScope: { kind: "default", minutes: 5 }, contextScopeSticky: true });
  });

  it("with Claude, explains the session already holds the whole meeting", () => {
    useConfigStore.setState({ llmProvider: "claude_cli" });
    render(<ContextScopePicker />);
    openPicker();
    expect(screen.getByText("Claude decides from the whole meeting")).toBeInTheDocument();
    expect(screen.getByText(/already holds the whole meeting/)).toBeInTheDocument();
  });

  it("with other providers, links to the per-action windows in Settings", async () => {
    useConfigStore.setState({ llmProvider: "openai" });
    const { useMeetingStore } = await import("../stores/meetingStore");
    useMeetingStore.setState({ currentView: "overlay", settingsOpen: false, settingsTab: null });
    render(<ContextScopePicker />);
    openPicker();
    expect(screen.getByText("Each action's own transcript window")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Settings → AI Actions" }));
    expect(useMeetingStore.getState()).toMatchObject({ settingsOpen: true, settingsTab: "ai_actions" });
    expect(screen.queryByRole("dialog", { name: "Transcript scope" })).not.toBeInTheDocument();
  });
});
