// Settings ↔ footer: one source of truth for the active LLM provider.
//
// Reported bug: "activating" Claude in Settings left the footer on the old
// provider until Claude was picked again in the footer.

import "@testing-library/jest-dom/vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeCliStatus } from "../lib/types";

vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: async () => null }));
vi.mock("@tauri-apps/plugin-store", () => ({
  load: async () => ({ set: async () => {}, get: async () => undefined, onKeyChange: async () => () => {} }),
}));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));

const ipc = vi.hoisted(() => ({
  claudeCliStatus: vi.fn(),
  claudeCliModels: vi.fn(),
  getAgentSessions: vi.fn(),
  setLLMProvider: vi.fn(),
  setActiveModel: vi.fn(),
  setClaudeCliConfig: vi.fn(),
}));

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/ipc")>()),
  claudeCliStatus: () => ipc.claudeCliStatus(),
  claudeCliModels: (...a: unknown[]) => ipc.claudeCliModels(...a),
  getAgentSessions: () => ipc.getAgentSessions(),
  setLLMProvider: (...a: unknown[]) => ipc.setLLMProvider(...a),
  setActiveModel: (...a: unknown[]) => ipc.setActiveModel(...a),
  setClaudeCliConfig: (...a: unknown[]) => ipc.setClaudeCliConfig(...a),
  getApiKey: async () => null,
  hasApiKey: async () => false,
  listModels: async () => [],
  listLocalSTTEngines: async () => [],
  getLLMProviders: async () => [],
}));

import { LLMSettings } from "./LLMSettings";
import { ServiceStatusBar } from "../components/ServiceStatusBar";
import { resetAgentStoreForTests } from "../stores/agentStore";
import { DEFAULT_CLAUDE_CLI_CONFIG, useConfigStore } from "../stores/configStore";

const LOGGED_IN: ClaudeCliStatus = {
  installed: true,
  binary_path: "C:\\Users\\me\\.local\\bin\\claude.exe",
  version: "2.1.288",
  auth: { logged_in: true, auth_method: "claude.ai", subscription_type: "max", email: "me@example.com", login: "subscription" },
  npm_only: false,
};

/** The footer's LLM chip: Claude's status chip, or the generic provider chip. */
function footerShowsClaude(): boolean {
  return screen.queryByTestId("agent-status-chip") !== null;
}

function claudeCard(): HTMLElement {
  return screen.getByRole("button", { name: /^Claude\b/ });
}

describe("Settings ↔ footer sync of the active LLM", () => {
  beforeEach(() => {
    resetAgentStoreForTests();
    useConfigStore.setState({
      llmProvider: "ollama",
      llmModel: "llama3.2",
      lastModelByProvider: { ollama: "llama3.2" },
      claudeCliConfig: DEFAULT_CLAUDE_CLI_CONFIG,
      claudeCliFallbackPinned: false,
      verifiedCloudProviders: [],
    });
    ipc.claudeCliStatus.mockReset().mockResolvedValue(LOGGED_IN);
    ipc.claudeCliModels.mockReset().mockResolvedValue([]);
    ipc.getAgentSessions.mockReset().mockResolvedValue([]);
    ipc.setLLMProvider.mockReset().mockResolvedValue(undefined);
    ipc.setActiveModel.mockReset().mockResolvedValue(undefined);
    ipc.setClaudeCliConfig.mockReset().mockResolvedValue(undefined);
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("the Claude panel in Settings activates Claude, and the footer follows", async () => {
    render(
      <>
        <LLMSettings />
        <ServiceStatusBar />
      </>,
    );
    expect(footerShowsClaude()).toBe(false);

    // Open Claude in Settings: its panel shows the login and models...
    fireEvent.click(claudeCard());
    await screen.findByText("me@example.com · Max");
    // ...and offers activation right there, not only at the bottom of the page.
    fireEvent.click(screen.getByRole("button", { name: "Use Claude" }));

    await waitFor(() => expect(footerShowsClaude()).toBe(true));
    expect(useConfigStore.getState().llmProvider).toBe("claude_cli");
    expect(ipc.setLLMProvider).toHaveBeenCalledWith(JSON.stringify({ provider_type: "claude_cli" }));
    expect(ipc.setActiveModel).toHaveBeenCalledWith("claude_cli", "haiku");
    expect(within(claudeCard()).getByText("Active")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Use Claude" })).not.toBeInTheDocument();
  });

  it("the bottom 'Set as Active' button takes the same path", async () => {
    render(
      <>
        <LLMSettings />
        <ServiceStatusBar />
      </>,
    );
    fireEvent.click(claudeCard());
    await screen.findByText("me@example.com · Max");
    fireEvent.click(screen.getByRole("button", { name: /set claude \(subscription\) as active provider/i }));
    await waitFor(() => expect(footerShowsClaude()).toBe(true));
    expect(ipc.setActiveModel).toHaveBeenCalledWith("claude_cli", "haiku");
  });

  it("a provider switched in the footer is reflected in open Settings", async () => {
    render(<LLMSettings />);
    // Settings opened while Ollama is active: Ollama is the selected card.
    expect(screen.getByRole("button", { name: /^Ollama/ })).toHaveAttribute("aria-pressed", "true");

    // The footer in the other window switches to Claude; the store's
    // cross-window sync (onKeyChange) applies it here.
    act(() => useConfigStore.setState({ llmProvider: "claude_cli", llmModel: "haiku" }));

    expect(screen.getByText(/Active: Claude \(subscription\)/)).toBeInTheDocument();
    // Settings shows the provider that is actually active...
    expect(claudeCard()).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /^Ollama/ })).toHaveAttribute("aria-pressed", "false");
    // ...and doesn't offer to "activate" the provider that was just replaced.
    expect(screen.queryByRole("button", { name: /set .* as active provider/i })).not.toBeInTheDocument();
  });
});
