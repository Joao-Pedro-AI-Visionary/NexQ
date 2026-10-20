// Footer chip for the Claude subscription provider: a status dot, the live
// session's state during a meeting, and a tooltip with model, effort and the
// meeting's cumulative usage. Rendered by ServiceStatusBar in both windows.

import { useEffect } from "react";
import { Brain, ChevronUp } from "lucide-react";
import { useAgentStore } from "../stores/agentStore";
import { useConfigStore } from "../stores/configStore";
import { useMeetingAgentUsage } from "../hooks/useMeetingAgentUsage";
import {
  API_BILLED_NOTE,
  NPM_ONLY_NOTE,
  formatCostEstimate,
  formatTokenUsage,
  isApiBilled,
  isRateLimited,
} from "../lib/agentUsage";
import { claudeTierLabel } from "../settings/ClaudeCliSettings";
import type { AgentSessionStatus, ClaudeCliConfig, ClaudeCliStatus, SessionUsage } from "../lib/types";

export type AgentChipState = "idle" | "starting" | "ready" | "busy" | "error" | "limited" | "attention";

// Color carries the state: green ready, amber starting / busy / needs the
// user / limit hit, red error, neutral unknown. Only starting and busy pulse.
const STATE_STYLE: Record<AgentChipState, { dot: string; pill: string; pulse: boolean }> = {
  ready: { dot: "bg-success", pill: "bg-success/10 border border-success/20", pulse: false },
  starting: { dot: "bg-warning", pill: "bg-warning/10 border border-warning/20", pulse: true },
  busy: { dot: "bg-warning", pill: "bg-warning/10 border border-warning/20", pulse: true },
  limited: { dot: "bg-warning", pill: "bg-warning/10 border border-warning/20", pulse: false },
  attention: { dot: "bg-warning", pill: "bg-warning/10 border border-warning/20", pulse: false },
  error: { dot: "bg-destructive", pill: "bg-destructive/10 border border-destructive/20", pulse: false },
  idle: { dot: "bg-muted-foreground/30", pill: "bg-secondary/30 border border-transparent", pulse: false },
};

function sessionState(sessions: AgentSessionStatus[]): { state: AgentChipState; label: string } {
  const errors = sessions.filter((s) => s.state === "error");
  if (errors.length > 0) {
    // A real fault outranks a limit on the other tier.
    return errors.every(isRateLimited)
      ? { state: "limited", label: "Usage limit" }
      : { state: "error", label: "Session error" };
  }
  if (sessions.some((s) => s.state === "busy")) return { state: "busy", label: "Thinking…" };
  if (sessions.some((s) => s.state === "starting")) return { state: "starting", label: "Starting…" };
  return { state: "ready", label: "Session ready" };
}

function providerState(
  status: ClaudeCliStatus | undefined,
  statusError: string | undefined,
): { state: AgentChipState; label: string } {
  if (!status) return { state: "idle", label: statusError ? "" : "Checking…" };
  if (!status.installed) return { state: "attention", label: status.npm_only ? "Needs native install" : "Not installed" };
  if (!status.auth) return { state: "idle", label: "" };
  if (!status.auth.logged_in) return { state: "attention", label: "Not logged in" };
  // Usable, but never green: it doesn't use the subscription.
  if (isApiBilled(status)) return { state: "attention", label: "API account" };
  return { state: "ready", label: "Ready" };
}

function providerNote(status: ClaudeCliStatus | undefined): string {
  if (status && !status.installed) return status.npm_only ? NPM_ONLY_NOTE : "Claude Code is not installed on this PC";
  if (status?.auth && !status.auth.logged_in) return "Not logged in — sign in from Settings";
  return "";
}

function buildTooltip(
  sessions: AgentSessionStatus[],
  usage: SessionUsage,
  cfg: ClaudeCliConfig,
  note: string,
  apiBilled: boolean,
): string {
  const tiers = sessions.length
    ? sessions
        .slice()
        .sort((a, b) => (a.tier === b.tier ? 0 : a.tier === "fast" ? -1 : 1))
        .map((s) => {
          const effort = s.tier === "fast" ? cfg.fast_effort : cfg.deep_effort;
          return `${s.tier === "fast" ? "Fast" : "Deep"}: ${claudeTierLabel(s.model, effort)}`;
        })
    : [
        `Fast: ${claudeTierLabel(cfg.fast_model, cfg.fast_effort)}`,
        `Deep: ${claudeTierLabel(cfg.deep_model, cfg.deep_effort)}`,
      ];
  const lines = [apiBilled ? "Claude (API account)" : "Claude (subscription)", ...tiers];
  if (apiBilled) lines.push(API_BILLED_NOTE);
  const detail = sessions.find((s) => s.state === "error" && s.detail)?.detail;
  if (detail) lines.push(detail);
  else if (note) lines.push(note);
  if (sessions.length > 0) lines.push(formatTokenUsage(usage), formatCostEstimate(usage, apiBilled));
  return lines.join("\n");
}

export function AgentStatusChip({
  compact = false,
  onClick,
  pickerOpen = false,
}: {
  compact?: boolean;
  onClick?: () => void;
  pickerOpen?: boolean;
}) {
  const status = useAgentStore((s) => s.status);
  const statusError = useAgentStore((s) => s.statusError);
  const cfg = useConfigStore((s) => s.claudeCliConfig);
  const { sessions, usage } = useMeetingAgentUsage();

  // Session updates only arrive once the store has subscribed (per window).
  useEffect(() => {
    const store = useAgentStore.getState();
    store.init();
    if (!store.status) void store.refreshStatus();
  }, []);

  // Closed sessions are dropped from the store, so any session here is open.
  const inSession = sessions.length > 0;
  const apiBilled = isApiBilled(status);
  const chip = inSession ? sessionState(sessions) : providerState(status, statusError);
  // An API-billed login is never shown green, in a session or not.
  const state: AgentChipState = apiBilled && chip.state === "ready" ? "attention" : chip.state;
  const label = chip.label;
  const style = STATE_STYLE[state];
  const tooltip = buildTooltip(sessions, usage, cfg, inSession ? "" : providerNote(status), apiBilled);

  return (
    <div
      data-testid="agent-status-chip"
      className={`flex items-center gap-1.5 rounded-full px-2.5 py-1 transition-colors duration-200 ${style.pill} ${
        onClick ? "cursor-pointer hover:brightness-110" : ""
      }`}
      title={tooltip}
      onClick={onClick}
    >
      <span className="relative flex h-2 w-2 shrink-0">
        {style.pulse && (
          <span className={`absolute inline-flex h-full w-full animate-pulse rounded-full ${style.dot} opacity-40`} />
        )}
        <span
          data-testid="agent-status-dot"
          data-state={state}
          className={`relative inline-flex h-2 w-2 rounded-full transition-colors duration-200 ${style.dot}`}
        />
      </span>
      <Brain className={`h-3.5 w-3.5 shrink-0 ${state === "idle" ? "text-muted-foreground/60" : "text-foreground/70"}`} />
      <div className="flex min-w-0 items-center gap-1">
        {!compact && <span className="text-xs font-medium text-foreground/90">Claude</span>}
        {label && (
          <span className={`truncate ${compact ? "text-xs font-medium text-foreground/90" : "text-meta text-foreground/60"}`}>
            {label}
          </span>
        )}
      </div>
      {onClick && (
        <ChevronUp
          className={`h-2.5 w-2.5 shrink-0 transition-all duration-200 ${
            pickerOpen ? "rotate-0 text-foreground/60" : "rotate-180 text-muted-foreground/50"
          }`}
        />
      )}
    </div>
  );
}
