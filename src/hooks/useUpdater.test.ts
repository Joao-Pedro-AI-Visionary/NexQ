import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (e: { payload: unknown }) => void;
const listeners = new Map<string, Set<Handler>>();
const emitted: string[] = [];

vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, cb: Handler) => {
    if (!listeners.has(name)) listeners.set(name, new Set());
    listeners.get(name)!.add(cb);
    return () => listeners.get(name)!.delete(cb);
  },
  emit: async (name: string) => {
    emitted.push(name);
  },
}));

const confirm = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: (...args: unknown[]) => confirm(...args),
}));

const installUpdateNow = vi.fn();
vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/ipc")>()),
  installUpdateNow: () => installUpdateNow(),
}));

import { restartToUpdate, MEETING_END_TIMEOUT_MS } from "./useUpdater";
import { useMeetingStore } from "../stores/meetingStore";
import { useToastStore } from "../stores/toastStore";

function windowEnded(window: string) {
  listeners.get("nexq:meeting_ended")?.forEach((cb) => cb({ payload: { window } }));
}

const flush = () => vi.advanceTimersByTimeAsync(0);

describe("restartToUpdate", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    listeners.clear();
    emitted.length = 0;
    confirm.mockReset();
    installUpdateNow.mockReset();
    installUpdateNow.mockResolvedValue(undefined);
    useToastStore.setState({ toasts: [] });
    useMeetingStore.setState({ activeMeeting: null });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("installs right away when no meeting is active", async () => {
    await restartToUpdate();

    expect(confirm).not.toHaveBeenCalled();
    expect(installUpdateNow).toHaveBeenCalledTimes(1);
  });

  it("does nothing when the user cancels the meeting prompt", async () => {
    useMeetingStore.setState({ activeMeeting: { id: "m1" } as never });
    confirm.mockResolvedValue(false);

    await restartToUpdate();

    expect(emitted).not.toContain("tray_stop_meeting");
    expect(installUpdateNow).not.toHaveBeenCalled();
  });

  it("ends the meeting in every window before installing", async () => {
    useMeetingStore.setState({ activeMeeting: { id: "m1" } as never });
    confirm.mockResolvedValue(true);

    const run = restartToUpdate();
    await flush();
    expect(emitted).toContain("tray_stop_meeting");

    windowEnded("launcher");
    await flush();
    expect(installUpdateNow).not.toHaveBeenCalled();

    windowEnded("overlay");
    await run;
    expect(installUpdateNow).toHaveBeenCalledTimes(1);
  });

  it("aborts the install and warns if a window doesn't finish ending the meeting", async () => {
    useMeetingStore.setState({ activeMeeting: { id: "m1" } as never });
    confirm.mockResolvedValue(true);

    const run = restartToUpdate();
    await flush();
    windowEnded("launcher");
    await vi.advanceTimersByTimeAsync(MEETING_END_TIMEOUT_MS);
    await run;

    expect(installUpdateNow).not.toHaveBeenCalled();
    expect(useToastStore.getState().toasts.some((t) => t.type === "error")).toBe(true);
  });
});
