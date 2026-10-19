import { beforeEach, describe, expect, it } from "vitest";
import { useUpdaterStore } from "./updaterStore";

const update = { version: "2.22.0", body: "Notes", date: null };

describe("updaterStore", () => {
  beforeEach(() => {
    useUpdaterStore.getState().reset();
    useUpdaterStore.setState({ skippedVersion: undefined, autoUpdate: true });
  });

  it("finishing a check with no update returns to idle and records lastChecked", () => {
    const store = useUpdaterStore.getState();
    store.startCheck();
    expect(useUpdaterStore.getState().phase).toBe("checking");

    store.finishCheck(null);

    const s = useUpdaterStore.getState();
    expect(s.phase).toBe("idle");
    expect(typeof s.lastChecked).toBe("number");
    expect(s.error).toBeUndefined();
  });

  it("accumulates download progress across chunks", () => {
    const store = useUpdaterStore.getState();
    store.startDownload();
    store.addProgress(100, 1000);
    store.addProgress(250, 1000);

    const s = useUpdaterStore.getState();
    expect(s.phase).toBe("downloading");
    expect(s.progress).toEqual({ received: 350, total: 1000 });
  });

  it("setReady marks the update ready", () => {
    useUpdaterStore.getState().setReady("2.22.0");

    const s = useUpdaterStore.getState();
    expect(s.phase).toBe("ready");
    expect(s.version).toBe("2.22.0");
  });

  it("auto checks ignore a skipped version but manual checks do not", () => {
    useUpdaterStore.setState({ skippedVersion: "2.22.0" });

    useUpdaterStore.getState().startCheck();
    const autoResult = useUpdaterStore.getState().finishCheck(update);
    expect(autoResult).toBeNull();
    expect(useUpdaterStore.getState().version).toBeUndefined();
    expect(useUpdaterStore.getState().phase).toBe("idle");

    useUpdaterStore.getState().startCheck();
    const manualResult = useUpdaterStore
      .getState()
      .finishCheck(update, { manual: true });
    expect(manualResult).toEqual(update);
    expect(useUpdaterStore.getState().version).toBe("2.22.0");
    expect(useUpdaterStore.getState().notes).toBe("Notes");
  });

  it("a failed check sets the error phase and message", () => {
    useUpdaterStore.getState().startCheck();
    useUpdaterStore.getState().failCheck("offline");

    const s = useUpdaterStore.getState();
    expect(s.phase).toBe("error");
    expect(s.error).toBe("offline");
  });
});
