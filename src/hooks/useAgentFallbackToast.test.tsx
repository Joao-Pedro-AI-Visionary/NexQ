import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (e: { payload: unknown }) => void;
const listeners = vi.hoisted(() => new Map<string, Set<(e: { payload: unknown }) => void>>());

vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: Handler) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name)!.add(cb);
    return () => listeners.get(name)!.delete(cb);
  },
}));
vi.mock("@tauri-apps/plugin-store", () => ({
  load: async () => ({ set: async () => {}, get: async () => undefined, onKeyChange: async () => () => {} }),
}));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));

import { fallbackMessage, useAgentFallbackToast } from "./useAgentFallbackToast";
import { useMeetingStore } from "../stores/meetingStore";
import { useToastStore } from "../stores/toastStore";

function Probe({ label }: { label: string }) {
  useAgentFallbackToast(label);
  return null;
}

async function emit(payload: unknown) {
  await act(async () => {
    listeners.get("agent_fallback")?.forEach((cb) => cb({ payload }));
  });
}

beforeEach(() => {
  listeners.clear();
  useToastStore.setState({ toasts: [] });
  useMeetingStore.setState({ currentView: "launcher" });
});
afterEach(cleanup);

describe("fallbackMessage", () => {
  it("uses the usage-limit copy for rate limits", () => {
    expect(fallbackMessage({ reason: "usage limit reached", provider: "Ollama" })).toBe(
      "Claude usage limit reached — using Ollama",
    );
  });
  it("names the reason otherwise", () => {
    expect(fallbackMessage({ reason: "not logged in", provider: "Groq" })).toBe(
      "Claude unavailable (not logged in) — using Groq",
    );
  });
});

describe("useAgentFallbackToast", () => {
  it("toasts a warning in the launcher outside a meeting", async () => {
    render(<Probe label="launcher" />);
    await act(async () => {});
    await emit({ reason: "usage limit reached", provider: "Ollama" });
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toMatchObject({ type: "warning", message: "Claude usage limit reached — using Ollama" });
  });

  it("leaves the toast to the overlay during a meeting", async () => {
    useMeetingStore.setState({ currentView: "overlay" });
    render(<Probe label="launcher" />);
    await act(async () => {});
    await emit({ reason: "not logged in", provider: "Groq" });
    expect(useToastStore.getState().toasts).toHaveLength(0);
  });

  it("toasts in the overlay during a meeting, not outside one", async () => {
    render(<Probe label="overlay" />);
    await act(async () => {});
    await emit({ reason: "not logged in", provider: "Groq" });
    expect(useToastStore.getState().toasts).toHaveLength(0);
    useMeetingStore.setState({ currentView: "overlay" });
    await emit({ reason: "not logged in", provider: "Groq" });
    expect(useToastStore.getState().toasts).toHaveLength(1);
  });
});
