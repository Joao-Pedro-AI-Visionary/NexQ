// Which part of the meeting a user action focuses on: the picker's setting
// (configStore) resolved into a concrete scope at request time.

import type { ContextScopeKind, ContextScopeSetting, ResolvedScope } from "./types";

export const DEFAULT_CONTEXT_SCOPE: ContextScopeSetting = { kind: "default", minutes: 5 };

/** One-click minute choices in the picker (a custom value is also allowed). */
export const MINUTE_PRESETS = [2, 5, 10, 15] as const;
export const MIN_MINUTES = 1;
export const MAX_MINUTES = 120;

/** Start times (epoch ms) of the previous user requests in this meeting. */
export interface LastUserRequests {
  anyAt: number | null;
  byMode: Record<string, number>;
}

export const NO_USER_REQUESTS: LastUserRequests = { anyAt: null, byMode: {} };

/** A whole number of minutes within 1–120 (5 when not a number). */
export function clampMinutes(minutes: number): number {
  if (!Number.isFinite(minutes)) return DEFAULT_CONTEXT_SCOPE.minutes;
  return Math.min(MAX_MINUTES, Math.max(MIN_MINUTES, Math.round(minutes)));
}

/** Picker order. */
export const SCOPE_KINDS: ContextScopeKind[] = ["default", "since_last_any", "since_last_same", "minutes", "all"];

/** Label on the picker chip and options. */
export function scopeChipLabel(setting: ContextScopeSetting): string {
  switch (setting.kind) {
    case "default":
      return "Action default";
    case "since_last_any":
      return "Since last answer";
    case "since_last_same":
      return "Since last same";
    case "minutes":
      return `Last ${clampMinutes(setting.minutes)} min`;
    case "all":
      return "Whole meeting";
  }
}

/** Short scope hint for an action button ("F/U · last 5 min"); null for the default. */
export function scopeHint(setting: ContextScopeSetting, actionLabel: string): string | null {
  switch (setting.kind) {
    case "default":
      return null;
    case "since_last_any":
      return "since last answer";
    case "since_last_same":
      return `since last ${actionLabel}`;
    case "minutes":
      return `last ${clampMinutes(setting.minutes)} min`;
    case "all":
      return "whole meeting";
  }
}

/**
 * The scope to send with a request for `mode`, or null for the action's
 * default (the backend then behaves exactly as before). "Since last …" with
 * no earlier request in this meeting falls back to the whole meeting.
 */
export function resolveScope(
  setting: ContextScopeSetting,
  mode: string,
  actionLabel: string,
  last: LastUserRequests,
  now: number,
): ResolvedScope | null {
  switch (setting.kind) {
    case "default":
      return null;
    case "all":
      return { kind: "all", since_ms: null, label: "Whole meeting" };
    case "minutes": {
      const minutes = clampMinutes(setting.minutes);
      return { kind: "minutes", since_ms: now - minutes * 60_000, minutes, label: `Last ${minutes} min` };
    }
    case "since_last_any":
      return last.anyAt != null
        ? { kind: "since_last_any", since_ms: last.anyAt, label: "Since last answer" }
        : { kind: "all", since_ms: null, label: "Whole meeting (first answer)" };
    case "since_last_same": {
      const at = last.byMode[mode];
      return at != null
        ? { kind: "since_last_same", since_ms: at, label: `Since last ${actionLabel}` }
        : { kind: "all", since_ms: null, label: `Whole meeting (first ${actionLabel})` };
    }
  }
}
