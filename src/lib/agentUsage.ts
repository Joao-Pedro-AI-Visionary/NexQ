// Pure helpers for Claude session status and usage, shared by the footer chip,
// the AI Log header and the fallback toast.

import type { AgentSessionStatus, ClaudeCliStatus, SessionUsage } from "./types";

/** Shown wherever a login that bills an API account would read as connected. */
export const API_BILLED_NOTE = "This Claude login bills an API account, not your subscription.";

/** Shown when only an npm install of Claude Code (`claude.cmd`) was found. */
export const NPM_ONLY_NOTE = "Found an npm install of Claude Code — NexQ needs the native installer";

/** The Claude login bills an API account (usable, but not the subscription). */
export function isApiBilled(status: ClaudeCliStatus | undefined | null): boolean {
  return status?.auth?.login === "api_billed";
}

const NUMBER = new Intl.NumberFormat("en-US");

export function formatTokens(n: number): string {
  return NUMBER.format(Math.round(n));
}

export function formatCost(usd: number): string {
  if (usd > 0 && usd < 0.005) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

/** "12,345 in / 6,789 out tokens" */
export function formatTokenUsage(u: SessionUsage): string {
  return `${formatTokens(u.input_tokens)} in / ${formatTokens(u.output_tokens)} out tokens`;
}

/** The cost line shown wherever usage appears. */
export function formatCostEstimate(u: SessionUsage, apiBilled = false): string {
  return `≈ ${formatCost(u.cost_usd)} estimate — ${apiBilled ? "billed to your API account" : "included in your subscription"}`;
}

/** Live-scope sessions of one meeting (fast and/or deep). */
export function liveSessionsOf(
  sessions: Record<string, AgentSessionStatus>,
  meetingId: string | null,
): AgentSessionStatus[] {
  if (!meetingId) return [];
  return Object.values(sessions).filter((s) => s.scope === "live" && s.meeting_id === meetingId);
}

/**
 * Cumulative usage of a meeting. Every session carries the usage of its whole
 * scope (both tiers), snapshotted when that session last changed, so tiers are
 * not summed: the larger snapshot per field is the latest one.
 */
export function scopeUsage(list: AgentSessionStatus[]): SessionUsage {
  return list.reduce<SessionUsage>(
    (acc, s) => ({
      input_tokens: Math.max(acc.input_tokens, s.usage.input_tokens),
      output_tokens: Math.max(acc.output_tokens, s.usage.output_tokens),
      cost_usd: Math.max(acc.cost_usd, s.usage.cost_usd),
    }),
    { input_tokens: 0, output_tokens: 0, cost_usd: 0 },
  );
}

/** Text fallback for a limit error when the backend sent no `error_kind`. */
export function isRateLimitText(text: string | null | undefined): boolean {
  if (!text) return false;
  const t = text.toLowerCase();
  return (
    t.includes("rate limit") ||
    t.includes("usage limit") ||
    (t.includes("limit reached") && !t.includes("context")) ||
    t.includes("too many requests") ||
    t.includes("per minute")
  );
}

/** The session stopped on a usage / rate limit (not a fault). */
export function isRateLimited(s: Pick<AgentSessionStatus, "error_kind" | "detail">): boolean {
  return s.error_kind != null ? s.error_kind === "rate_limited" : isRateLimitText(s.detail);
}
