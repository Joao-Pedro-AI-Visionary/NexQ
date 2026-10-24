import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, Play, X } from "lucide-react";
import { dismissMeetingPrompt, getDetectedMeeting } from "../lib/ipc";
import { emitStartDetectedMeeting, onDetectedMeetingEnded, onMeetingDetected } from "../lib/events";
import { composeMeetingTitle } from "../lib/meetingTitle";
import { BUILT_IN_SCENARIOS } from "../lib/scenarios";
import type { AIScenario, MeetingPromptInfo } from "../lib/types";

/**
 * The meeting-detection prompt: a small always-on-top card at the top of the
 * screen when Zoom, Teams, Meet… start a call. Start opens a NexQ meeting
 * named after the call; it goes away on its own after a while.
 */
export function MeetingPrompt({
  info,
  onStart,
  onDismiss,
}: {
  info: MeetingPromptInfo;
  onStart: (title: string, scenario: AIScenario) => void;
  onDismiss: () => void;
}) {
  const { meeting } = info;
  const [title, setTitle] = useState(meeting.title ?? "");
  const [scenario, setScenario] = useState<AIScenario>(info.default_scenario);
  const [paused, setPaused] = useState(false);
  const startRef = useRef<HTMLButtonElement>(null);
  const scenarioName = BUILT_IN_SCENARIOS.find((s) => s.id === scenario)?.name ?? "Meeting";

  const start = useCallback(() => {
    onStart(composeMeetingTitle(title.trim() || meeting.app_name), scenario);
  }, [meeting.app_name, onStart, scenario, title]);

  // Unanswered: goes away after auto_dismiss_secs (paused while hovered or focused).
  const total = info.auto_dismiss_secs * 1000;
  const [left, setLeft] = useState(total);
  useEffect(() => {
    if (total <= 0 || paused) return;
    const step = 100;
    const id = window.setInterval(() => setLeft((l) => Math.max(0, l - step)), step);
    return () => window.clearInterval(id);
  }, [paused, total]);
  useEffect(() => {
    if (total > 0 && left === 0) onDismiss();
  }, [left, onDismiss, total]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onDismiss();
      else if (e.key === "Enter" && (e.target as HTMLElement)?.tagName !== "SELECT") start();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onDismiss, start]);

  return (
    <div className="flex h-screen w-screen items-start justify-center p-2">
      <div
        role="dialog"
        aria-label={`${meeting.app_name} meeting detected`}
        onMouseEnter={() => setPaused(true)}
        onMouseLeave={() => setPaused(false)}
        onFocus={() => setPaused(true)}
        className="meeting-prompt-enter relative w-full overflow-hidden rounded-[18px] border border-white/10 bg-[hsl(228_22%_9%/0.96)] text-foreground shadow-[0_10px_30px_-8px_rgba(0,0,0,0.6)] backdrop-blur-xl"
      >
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss"
          className="absolute left-1.5 top-1.5 flex h-5 w-5 items-center justify-center rounded-full text-muted-foreground/60 transition-colors hover:bg-white/10 hover:text-foreground cursor-pointer"
        >
          <X className="h-3 w-3" />
        </button>

        <div className="flex items-center gap-3 py-3 pl-8 pr-3">
          <img src="/nexq-icon.png" alt="" className="h-9 w-9 shrink-0 rounded-xl" />
          <div className="min-w-0 flex-1">
            <p className="flex items-center gap-1.5 truncate whitespace-nowrap text-[11px] font-medium text-muted-foreground">
              <span className="relative flex h-1.5 w-1.5 shrink-0" aria-hidden="true">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-success opacity-60 motion-reduce:hidden" />
                <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-success" />
              </span>
              <span className="truncate">
                {meeting.app_name} meeting detected
                <span className="text-muted-foreground/50"> · {scenarioName}</span>
              </span>
            </p>
            <input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder={`${meeting.app_name} — name this meeting`}
              aria-label="Meeting name"
              maxLength={120}
              className="mt-0.5 w-full truncate bg-transparent text-sm font-semibold text-foreground placeholder:font-medium placeholder:text-muted-foreground/50 focus:outline-none"
            />
          </div>

          {/* Split button: Start, and the meeting type behind the chevron */}
          <div className="flex shrink-0 items-stretch overflow-hidden rounded-xl bg-primary text-primary-foreground shadow-sm shadow-primary/30">
            <button
              ref={startRef}
              type="button"
              onClick={start}
              className="flex items-center gap-1.5 py-2 pl-3.5 pr-3 text-xs font-semibold transition-all duration-150 hover:brightness-110 active:scale-[0.97] cursor-pointer"
            >
              <Play className="h-3 w-3 fill-current" aria-hidden="true" />
              Start NexQ
            </button>
            <label className="relative flex w-7 cursor-pointer items-center justify-center border-l border-primary-foreground/20 transition-colors hover:bg-black/10">
              <ChevronDown className="pointer-events-none h-3.5 w-3.5" aria-hidden="true" />
              <select
                value={scenario}
                onChange={(e) => setScenario(e.target.value as AIScenario)}
                aria-label={`Meeting type: ${scenarioName}`}
                className="absolute inset-0 cursor-pointer appearance-none opacity-0"
              >
                {BUILT_IN_SCENARIOS.map((s) => (
                  <option key={s.id} value={s.id} className="bg-neutral-900 text-foreground">
                    {s.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </div>

        <p className="-mt-1.5 truncate pb-2.5 pl-8 pr-3 text-[10px] text-muted-foreground/45">
          Let everyone know the meeting is being transcribed.
        </p>

        {total > 0 && (
          <div className="absolute inset-x-0 bottom-0 h-[2px] bg-white/5" aria-hidden="true">
            <div className="h-full bg-primary/50 transition-[width] duration-100 ease-linear" style={{ width: `${(left / total) * 100}%` }} />
          </div>
        )}
      </div>
    </div>
  );
}

/** Root of the `meeting-prompt` window. */
export function MeetingPromptWindow() {
  const [info, setInfo] = useState<MeetingPromptInfo | null>(null);
  // Remount the card (fresh title, timer) for every new call.
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    document.documentElement.classList.add("dark");
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";

    // Dev only: `?view=meeting-prompt&demo` previews the card in a browser.
    if (window.location.port === "5173" && new URLSearchParams(window.location.search).has("demo")) {
      setInfo({ meeting: { app: "zoom", app_name: "Zoom", title: "Weekly design sync" }, auto_dismiss_secs: 60, default_scenario: "team_meeting" });
      return;
    }
    getDetectedMeeting().then((current) => current && setInfo(current)).catch(() => {});
    const unlisteners: Array<() => void> = [];
    onMeetingDetected((next) => {
      setInfo(next);
      setGeneration((g) => g + 1);
    }).then((fn) => unlisteners.push(fn));
    onDetectedMeetingEnded(() => setInfo(null)).then((fn) => unlisteners.push(fn));
    return () => unlisteners.forEach((fn) => fn());
  }, []);

  const dismiss = useCallback(() => {
    setInfo(null);
    dismissMeetingPrompt().catch(() => {});
  }, []);

  const start = useCallback((title: string, scenario: AIScenario) => {
    emitStartDetectedMeeting({ title, scenario }).catch(() => {});
    dismiss();
  }, [dismiss]);

  if (!info) return null;
  return <MeetingPrompt key={generation} info={info} onStart={start} onDismiss={dismiss} />;
}
