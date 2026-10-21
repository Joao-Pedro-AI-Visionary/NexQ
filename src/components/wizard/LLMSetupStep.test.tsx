import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClaudeCliStatus } from "../../lib/types";

vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: async () => null }));
vi.mock("@tauri-apps/plugin-store", () => ({
  load: async () => ({ set: async () => {}, get: async () => undefined, onKeyChange: async () => () => {} }),
}));

const ipc = vi.hoisted(() => ({ claudeCliStatus: vi.fn() }));
vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/ipc")>()),
  claudeCliStatus: () => ipc.claudeCliStatus(),
  listModels: async () => [],
}));

import { LLMSetupStep } from "./LLMSetupStep";
import { resetAgentStoreForTests } from "../../stores/agentStore";
import { useConfigStore } from "../../stores/configStore";

const LOGGED_IN: ClaudeCliStatus = {
  installed: true,
  binary_path: "claude.exe",
  version: "2.1.0",
  auth: { logged_in: true, auth_method: "claude.ai", subscription_type: "max", email: "me@example.com", login: "subscription" },
  npm_only: false,
};

describe("LLMSetupStep (first-run wizard)", () => {
  const activate = vi.fn(async () => {});

  beforeEach(() => {
    resetAgentStoreForTests();
    ipc.claudeCliStatus.mockReset().mockResolvedValue(LOGGED_IN);
    activate.mockClear();
    useConfigStore.setState({ llmProvider: "ollama", llmModel: "", activateLLMProvider: activate });
  });

  afterEach(() => cleanup());

  it("activates Claude through the shared activation action", async () => {
    render(<LLMSetupStep ollamaRunning={false} ollamaModels={[]} lmStudioRunning={false} lmStudioModels={[]} />);
    fireEvent.click(await screen.findByRole("button", { name: /^Claude/ }));
    fireEvent.click(screen.getByRole("button", { name: "Use Claude" }));
    await waitFor(() => expect(activate).toHaveBeenCalledWith("claude_cli"));
  });
});
