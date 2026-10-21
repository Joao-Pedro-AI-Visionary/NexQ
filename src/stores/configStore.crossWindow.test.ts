// Two windows (launcher + overlay) each have their own configStore. They meet
// in the plugin-store: a write in one window emits `store://change` to every
// window, and `onKeyChange` applies it. This models that with one shared map
// and a broadcast to every window's listeners.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const saved = vi.hoisted(() => new Map<string, unknown>());
const keyListeners = vi.hoisted(() => [] as Array<{ key: string; cb: (v: unknown) => void }>);
const invoke = vi.hoisted(() => vi.fn(async (_cmd: string, _args?: unknown): Promise<unknown> => null));

vi.mock("@tauri-apps/plugin-store", () => ({
  load: async () => ({
    get: async (key: string) => saved.get(key),
    set: async (key: string, value: unknown) => {
      saved.set(key, value);
      // Like the Rust side: every window (the writer included) hears the change.
      for (const l of keyListeners) if (l.key === key) l.cb(structuredClone(value));
    },
    onKeyChange: async (key: string, cb: (v: unknown) => void) => {
      keyListeners.push({ key, cb });
      return () => {};
    },
  }),
}));

vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: unknown) => invoke(cmd, args) }));

/** A window: a fresh module graph (own Zustand store) that has loaded its config. */
async function openWindow() {
  vi.resetModules();
  const { useConfigStore } = await import("./configStore");
  await useConfigStore.getState().loadConfig();
  return useConfigStore;
}

describe("configStore: active LLM across windows", () => {
  beforeEach(() => {
    saved.clear();
    keyListeners.length = 0;
    invoke.mockClear();
    saved.set("llmProvider", "ollama");
    saved.set("llmModel", "llama3.2");
    vi.spyOn(console, "log").mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("a provider set in one window reaches the other", async () => {
    const launcher = await openWindow();
    const overlay = await openWindow();
    expect(overlay.getState().llmProvider).toBe("ollama");

    launcher.getState().setLLMProvider("claude_cli");
    launcher.getState().setLLMModel("haiku", "claude_cli");

    await vi.waitFor(() => expect(overlay.getState().llmProvider).toBe("claude_cli"));
    expect(overlay.getState().llmModel).toBe("haiku");
  });

  it("the Claude model catalog fetched by the launcher reaches the overlay and the next start", async () => {
    const launcher = await openWindow();
    const overlay = await openWindow();
    expect(overlay.getState().claudeModels).toBeNull();
    const catalog = [
      {
        value: "haiku",
        resolved_model: "claude-haiku-4-5-20251001",
        display_name: "Haiku 4.5",
        description: "",
        supports_effort: false,
        effort_levels: [],
      },
    ];
    launcher.getState().setClaudeModels(catalog);
    await vi.waitFor(() => expect(overlay.getState().claudeModels).toEqual(catalog));
    const nextStart = await openWindow();
    expect(nextStart.getState().claudeModels).toEqual(catalog);
  });

  it("activating in either window updates the backend and both windows", async () => {
    const launcher = await openWindow();
    const overlay = await openWindow();

    // Settings (launcher) → Claude.
    await launcher.getState().activateLLMProvider("claude_cli");
    expect(invoke).toHaveBeenCalledWith("set_llm_provider", { provider: JSON.stringify({ provider_type: "claude_cli" }) });
    expect(invoke).toHaveBeenCalledWith("set_active_model", { provider: "claude_cli", modelId: "haiku" });
    await vi.waitFor(() => expect(overlay.getState().llmProvider).toBe("claude_cli"));

    // Footer (overlay) → back to Ollama with its last model.
    await overlay.getState().activateLLMProvider("ollama");
    await vi.waitFor(() => expect(launcher.getState().llmProvider).toBe("ollama"));
    expect(launcher.getState().llmModel).toBe("llama3.2");
    expect(invoke).toHaveBeenLastCalledWith("set_active_model", { provider: "ollama", modelId: "llama3.2" });
  });
});
