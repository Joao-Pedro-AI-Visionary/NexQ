// "Hearing audio but no transcript": a party whose audio is clearly coming in
// while its STT provider returns nothing (missing model, broken provider,
// provider that can't read that source). Without a warning this looks like a
// meeting where nobody spoke.

import type { AudioSource } from "./types";

export type Party = "you" | "them";

/** Backend level (RMS / 3000, 0–1) that counts as audio worth transcribing. */
export const HEARD_LEVEL = 0.05;
/** Audio heard without any transcript before the warning shows. */
export const NO_TRANSCRIPT_AFTER_MS = 30_000;
/** Longest gap between two level events still counted as continuous audio. */
const MAX_STEP_MS = 1_000;

export interface PartyWatch {
  /** Time this party's audio was at or above {@link HEARD_LEVEL}. */
  heardMs: number;
  /** When the last level event arrived. */
  lastAt: number | null;
}

export const EMPTY_WATCH: PartyWatch = { heardMs: 0, lastAt: null };

/** Fold one audio level event into a party's watch. */
export function addLevel(watch: PartyWatch, level: number, now: number): PartyWatch {
  const step = watch.lastAt == null ? 0 : Math.min(Math.max(now - watch.lastAt, 0), MAX_STEP_MS);
  return { heardMs: watch.heardMs + (level >= HEARD_LEVEL ? step : 0), lastAt: now };
}

/** Which party's provider transcribes an audio source (room audio goes to "Them"). */
export function partyOfSource(source: AudioSource): Party {
  return source === "Mic" ? "you" : "them";
}

/** Which party a transcript line came from. */
export function partyOfSpeaker(speaker: string): Party {
  return speaker === "User" ? "you" : "them";
}

/** Whether to warn: plenty of audio, no transcript, not muted. */
export function shouldWarn(watch: PartyWatch, transcribed: boolean, muted: boolean): boolean {
  return !transcribed && !muted && watch.heardMs >= NO_TRANSCRIPT_AFTER_MS;
}
