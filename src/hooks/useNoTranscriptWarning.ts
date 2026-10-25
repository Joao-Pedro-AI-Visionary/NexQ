// Parties heard for a while this meeting without any transcript (see
// lib/sttWatchdog.ts). Logged once per party so a bug report's log shows it.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { onAudioLevel } from "../lib/events";
import { logToFile } from "../lib/logger";
import {
  EMPTY_WATCH,
  NO_TRANSCRIPT_AFTER_MS,
  addLevel,
  partyOfSource,
  partyOfSpeaker,
  shouldWarn,
  type Party,
  type PartyWatch,
} from "../lib/sttWatchdog";
import { useConfigStore } from "../stores/configStore";
import { useMeetingStore } from "../stores/meetingStore";
import { useTranscriptStore } from "../stores/transcriptStore";

const NONE: Record<Party, boolean> = { you: false, them: false };

export function useNoTranscriptWarning(): { parties: Party[]; dismiss: (party: Party) => void } {
  const isRecording = useMeetingStore((s) => s.isRecording);
  const meetingId = useMeetingStore((s) => s.activeMeeting?.id ?? null);
  const segments = useTranscriptStore((s) => s.segments);
  const mutedYou = useConfigStore((s) => s.mutedYou);
  const mutedThem = useConfigStore((s) => s.mutedThem);
  const meetingAudioConfig = useConfigStore((s) => s.meetingAudioConfig);

  const watches = useRef<Record<Party, PartyWatch>>({ you: EMPTY_WATCH, them: EMPTY_WATCH });
  // Flips once a party's heard time crosses the threshold (re-renders the panel).
  const [heard, setHeard] = useState(NONE);
  const [dismissed, setDismissed] = useState(NONE);
  const logged = useRef(NONE);

  useEffect(() => {
    watches.current = { you: EMPTY_WATCH, them: EMPTY_WATCH };
    logged.current = NONE;
    setHeard(NONE);
    setDismissed(NONE);
  }, [meetingId]);

  useEffect(() => {
    if (!isRecording) return;
    let unlisten: (() => void) | undefined;
    let alive = true;
    onAudioLevel((event) => {
      const party = partyOfSource(event.source);
      const watch = addLevel(watches.current[party], event.level, Date.now());
      watches.current[party] = watch;
      if (watch.heardMs >= NO_TRANSCRIPT_AFTER_MS) {
        setHeard((h) => (h[party] ? h : { ...h, [party]: true }));
      }
    }).then((fn) => (alive ? (unlisten = fn) : fn()));
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [isRecording, meetingId]);

  const transcribed = useMemo(() => {
    const out = { ...NONE };
    for (const segment of segments) out[partyOfSpeaker(segment.speaker)] = true;
    return out;
  }, [segments]);

  const parties = (["you", "them"] as Party[]).filter(
    (party) =>
      isRecording &&
      heard[party] &&
      !dismissed[party] &&
      shouldWarn(watches.current[party], transcribed[party], party === "you" ? mutedYou : mutedThem),
  );

  const key = parties.join(",");
  useEffect(() => {
    for (const party of parties) {
      if (logged.current[party]) continue;
      logged.current = { ...logged.current, [party]: true };
      const provider = meetingAudioConfig?.[party].stt_provider ?? "unknown";
      logToFile(
        "warn",
        "transcript",
        `'${party === "you" ? "You" : "Them"}' audio heard for ${NO_TRANSCRIPT_AFTER_MS / 1000}s but ${provider} returned no transcript`,
      );
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const dismiss = useCallback((party: Party) => setDismissed((d) => ({ ...d, [party]: true })), []);
  return { parties, dismiss };
}
