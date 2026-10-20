import "@testing-library/jest-dom/vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentSessionStatus, Meeting } from "../lib/types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => () => {},
}));
vi.mock("@tauri-apps/plugin-store", () => ({
  load: async () => ({ set: async () => {}, get: async () => undefined, onKeyChange: async () => () => {} }),
}));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));

const ipc = vi.hoisted(() => ({
  claudeCliStatus: vi.fn(),
  getAgentSessions: vi.fn(),
}));
vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/ipc")>()),
  claudeCliStatus: () => ipc.claudeCliStatus(),
  getAgentSessions: () => ipc.getAgentSessions(),
}));

import { AgentStatusChip, scopeUsage } from "./AgentStatusChip";
import { agentSessionKey, resetAgentStoreForTests, useAgentStore } from "../stores/agentStore";
import { useMeetingStore } from "../stores/meetingStore";

function session(over: Partial<AgentSessionStatus> = {}): AgentSessionStatus {
  return {
    scope: "live",
    meeting_id: "m1",
    tier: "fast",
    state: "ready",
    model: "haiku",
    detail: null,
    usage: { input_tokens: 12345, output_tokens: 678, cost_usd: 0.1234 },
    ...over,
  };
}

function setSessions(...list: AgentSessionStatus[]) {
  useAgentStore.setState({ sessions: Object.fromEntries(list.map((s) => [agentSessionKey(s), s])) });
}

beforeEach(() => {
  resetAgentStoreForTests();
  ipc.getAgentSessions.mockResolvedValue([]);
  ipc.claudeCliStatus.mockResolvedValue({
    installed: true,
    binary_path: "claude",
    version: "2.1.0",
    auth: { logged_in: true, auth_method: "oauth", subscription_type: "max", email: "a@b.c" },
  });
  useMeetingStore.setState({ activeMeeting: { id: "m1" } as Meeting });
});

afterEach(() => {
  cleanup();
  useMeetingStore.setState({ activeMeeting: null });
});

describe("AgentStatusChip", () => {
  it("shows a ready dot and 'Session ready' for the live session", async () => {
    render(<AgentStatusChip />);
    await act(async () => setSessions(session()));
    expect(screen.getByTestId("agent-status-dot")).toHaveAttribute("data-state", "ready");
    expect(screen.getByText("Session ready")).toBeInTheDocument();
  });

  it("shows amber 'Thinking…' while a turn is running", async () => {
    render(<AgentStatusChip />);
    await act(async () => setSessions(session({ state: "busy" })));
    expect(screen.getByTestId("agent-status-dot")).toHaveAttribute("data-state", "busy");
    expect(screen.getByText("Thinking…")).toBeInTheDocument();
  });

  it("marks the error state", async () => {
    render(<AgentStatusChip />);
    await act(async () => setSessions(session({ state: "error", detail: "process crashed" })));
    expect(screen.getByTestId("agent-status-dot")).toHaveAttribute("data-state", "error");
    expect(screen.getByTestId("agent-status-chip").title).toContain("process crashed");
  });

  it("turns amber, not red, on a usage limit", async () => {
    render(<AgentStatusChip />);
    await act(async () => setSessions(session({ state: "error", detail: "5-hour usage limit reached" })));
    expect(screen.getByTestId("agent-status-dot")).toHaveAttribute("data-state", "limited");
  });

  it("lists model, effort and thousands-separated tokens in the tooltip", async () => {
    render(<AgentStatusChip />);
    await act(async () => setSessions(session()));
    const tip = screen.getByTestId("agent-status-chip").title;
    expect(tip).toContain("12,345 in / 678 out tokens");
    expect(tip).toContain("≈ $0.12 estimate — included in your subscription");
    expect(tip).toContain("Fast: Haiku · Low");
  });

  it("outside a meeting shows the provider status", async () => {
    useMeetingStore.setState({ activeMeeting: null });
    render(<AgentStatusChip />);
    await act(async () => {});
    expect(screen.getByTestId("agent-status-dot")).toHaveAttribute("data-state", "ready");
    expect(screen.getByText("Ready")).toBeInTheDocument();
  });

  it("ignores sessions of other meetings and scopes", async () => {
    useMeetingStore.setState({ activeMeeting: null });
    render(<AgentStatusChip />);
    await act(async () => setSessions(session({ meeting_id: "other", state: "busy" })));
    expect(screen.queryByText("Thinking…")).not.toBeInTheDocument();
  });
});

describe("scopeUsage", () => {
  it("does not double count the scope-wide usage carried by each tier", () => {
    const u = { input_tokens: 100, output_tokens: 10, cost_usd: 0.5 };
    const total = scopeUsage([session({ usage: u }), session({ tier: "deep", usage: { ...u, input_tokens: 90 } })]);
    expect(total).toEqual(u);
  });
});
