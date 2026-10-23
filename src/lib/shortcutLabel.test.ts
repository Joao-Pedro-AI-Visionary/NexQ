import { describe, expect, it } from "vitest";
import { acceleratorFromEvent, formatShortcut } from "./shortcutLabel";

const press = (over: Partial<KeyboardEvent>) =>
  ({ key: "h", code: "KeyH", ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...over }) as KeyboardEvent;

describe("formatShortcut", () => {
  it("reads as Ctrl on Windows and as symbols on macOS", () => {
    expect(formatShortcut("CmdOrCtrl+Shift+H", false)).toBe("Ctrl+Shift+H");
    expect(formatShortcut("CmdOrCtrl+Shift+H", true)).toBe("⌘⇧H");
    expect(formatShortcut("Ctrl+Alt+K", true)).toBe("⌃⌥K");
  });
});

describe("acceleratorFromEvent", () => {
  it("maps Ctrl and Cmd to CmdOrCtrl and uses the physical key", () => {
    expect(acceleratorFromEvent(press({ ctrlKey: true, shiftKey: true, key: "H" }))).toBe("CmdOrCtrl+Shift+H");
    expect(acceleratorFromEvent(press({ metaKey: true, altKey: true, key: "˙" }))).toBe("CmdOrCtrl+Alt+H");
    expect(acceleratorFromEvent(press({ ctrlKey: true, code: "Digit7", key: "7" }))).toBe("CmdOrCtrl+7");
  });

  it("rejects lone modifiers, plain keys and unsupported keys", () => {
    expect(acceleratorFromEvent(press({ ctrlKey: true, key: "Control", code: "ControlLeft" }))).toBeNull();
    expect(acceleratorFromEvent(press({ shiftKey: true }))).toBeNull();
    expect(acceleratorFromEvent(press({ ctrlKey: true, key: "Enter", code: "Enter" }))).toBeNull();
  });
});
