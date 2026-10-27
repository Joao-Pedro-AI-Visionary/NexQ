import { useEffect, useState, useRef, useCallback } from "react";
import { HelpCircle, Sparkles, Check, Clock, X } from "lucide-react";
import { onQuestionDetected, onStreamEnd } from "../lib/events";
import { runUserAction } from "../lib/userAction";
import { logToFile } from "../lib/logger";
import { useTranscriptStore } from "../stores/transcriptStore";
import { useMeetingStore } from "../stores/meetingStore";
import { useStreamStore } from "../stores/streamStore";
import { useAIActionsStore } from "../stores/aiActionsStore";
import type { DetectedQuestion } from "../lib/types";

export function looksLikeQuestion(text: string): boolean {
  const trimmed = text.trim();
  if (trimmed.endsWith("?")) return true;
  const lower = trimmed.toLowerCase();
  const qWords = [
    "what ", "how ", "why ", "when ", "where ", "who ", "which ",
    "can you", "could you", "would you", "do you", "are you",
    "is there", "have you", "tell me", "explain",
    // pt-BR
    "o que ", "o quê", "qual ", "quais ", "quanto ", "quanta ", "quantos ", "quantas ",
    "por que ", "quando ", "onde ", "aonde ", "quem ",
    "me conta", "me fala", "me diga", "me explica", "conta pra mim", "fala um pouco",
    "fale sobre", "descreva", "explique", "pode me ", "poderia ", "você consegue", "voce consegue",
    "e você", "e voce",
  ];
  if (qWords.some((w) => lower.startsWith(w))) return true;
  // pt-BR prompts that are questions even without "?"
  const ptPatterns = [
    "me conta", "me fala", "me explica", "fala um pouco", "conta um pouco", "me dá um exemplo",
    "me de um exemplo", "qual a sua", "qual é a sua", "qual sua", "como você", "como voce",
    "o que você", "o que voce", "por que você", "por que voce", "você já", "voce ja", "você tem", "voce tem",
  ];
  return ptPatterns.some((w) => lower.includes(w));
}

/**
 * The newest sentence of a transcript line. Lines grow into paragraphs, so a
 * question is usually the last sentence, not the whole line.
 */
export function lastSentence(text: string): string {
  const sentences = text.trim().match(/[^.?!]+[.?!]*/g) ?? [];
  return (sentences[sentences.length - 1] ?? "").trim();
}

/**
 * Newest question in a transcript line. Whisper often appends filler like
 * "E aí" or "Tchau" after a question (silence hallucinations), so the last
 * sentence alone misses it: scan back for the latest question-like sentence.
 */
export function latestQuestion(text: string): string {
  const sentences = text.trim().match(/[^.?!]+[.?!]*/g) ?? [];
  for (let i = sentences.length - 1; i >= 0; i--) {
    const s = sentences[i].trim();
    if (s.split(/\s+/).length >= 3 && looksLikeQuestion(s)) return s;
  }
  return "";
}

interface TrackedQuestion extends DetectedQuestion {
  assisted: boolean;
  /** Transcript line the question came from (lines grow while someone talks). */
  segmentId?: string;
}

export function QuestionDetector() {
  const [questions, setQuestions] = useState<TrackedQuestion[]>([]);
  // Text of each final line when it was last checked for a question.
  const checkedTextRef = useRef<Map<string, string>>(new Map());
  const segments = useTranscriptStore((s) => s.segments);
  const activeMeetingId = useMeetingStore((s) => s.activeMeeting?.id ?? "");

  // Clear detected questions when a new meeting starts so a question from the
  // previous meeting doesn't linger in the overlay.
  useEffect(() => {
    setQuestions([]);
    checkedTextRef.current = new Map();
  }, [activeMeetingId]);

  // Auto-assist: when the other party asks a question and auto-trigger is on,
  // run Assist by itself after a short pause (no click needed), like Persua.
  const autoTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastAutoRef = useRef<{ text: string; at: number }>({ text: "", at: 0 });
  const scheduleAutoAssist = useCallback((text: string) => {
    const enabled = useAIActionsStore.getState().configs.globalDefaults.autoTrigger;
    if (!enabled || !text || text.trim().length < 8) return;
    if (autoTimerRef.current) clearTimeout(autoTimerRef.current);
    autoTimerRef.current = setTimeout(() => {
      autoTimerRef.current = null;
      const now = Date.now();
      const last = lastAutoRef.current;
      if (last.text === text || now - last.at < 6000) return;
      const st = useStreamStore.getState();
      if (st.isStreaming || st.phase === "sending") return;
      lastAutoRef.current = { text, at: now };
      logToFile("info", "auto-assist", `pergunta detectada, rodando Assist: "${text}"`);
      setQuestions((prev) => prev.map((p) => (p.text === text ? { ...p, assisted: true } : p)));
      runUserAction("Assist", text).catch(() => {});
    }, 1200);
  }, []);
  useEffect(() => () => { if (autoTimerRef.current) clearTimeout(autoTimerRef.current); }, []);

  // Log each AI answer (short) so the diagnostics log shows question -> answer.
  useEffect(() => {
    const p = onStreamEnd((e) => {
      setTimeout(() => {
        const st = useStreamStore.getState();
        const text = (st.currentContent || "").replace(/\s+/g, " ").trim();
        if (text) logToFile("info", "auto-assist", `resposta IA (${st.currentMode ?? "?"}, ${e.latency_ms ?? "?"}ms): ${text.slice(0, 400)}`);
      }, 300);
    });
    return () => { p.then((u) => u()); };
  }, []);

  const addQuestion = useCallback((q: DetectedQuestion, segmentId?: string) => {
    scheduleAutoAssist(q.text);
    setQuestions((prev) => {
      // The same line grew: update its question in place (unless already answered).
      if (segmentId) {
        const i = prev.findIndex((p) => p.segmentId === segmentId);
        if (i >= 0) {
          if (prev[i].assisted || prev[i].text === q.text) return prev;
          return prev.map((p, j) => (j === i ? { ...p, text: q.text } : p));
        }
      }
      if (prev.length > 0 && prev[0].text === q.text) return prev;
      return [{ ...q, assisted: false, segmentId }, ...prev].slice(0, 10);
    });
  }, [scheduleAutoAssist]);

  useEffect(() => {
    const p = onQuestionDetected((event) => {
      if (event.source === "Them" || event.source === "Interviewer") {
        addQuestion(event);
      }
    });
    return () => { p.then((u) => u()); };
  }, [addQuestion]);

  useEffect(() => {
    const checked = checkedTextRef.current;
    for (const seg of segments) {
      // Groq batches return complete 5 s chunks; Windows loopback stops sending
      // audio during silence, so a 'Them' line may never be finalized. Check
      // partial text too (only the newly added part is analysed).
      if (!seg.text || checked.get(seg.id) === seg.text) continue;
      const prev = checked.get(seg.id) ?? "";
      checked.set(seg.id, seg.text);
      if (seg.speaker !== "Them" && seg.speaker !== "Interviewer") continue;
      // Only look at what was added since the last check (lines grow).
      const added = prev && seg.text.startsWith(prev) ? seg.text.slice(prev.length) : seg.text;
      const sentence = latestQuestion(added);
      if (sentence) {
        addQuestion({ text: sentence, confidence: 0.8, timestamp_ms: seg.timestamp_ms, source: seg.speaker }, seg.id);
      }
    }
  }, [segments, addQuestion]);

  const handleAssist = useCallback((index: number) => {
    const questionText = questions[index]?.text;
    setQuestions((prev) =>
      prev.map((q, i) => i === index ? { ...q, assisted: true } : q)
    );
    runUserAction("Assist", questionText).catch(() => {});
  }, [questions]);

  const handleDismiss = useCallback((index: number, e: React.MouseEvent) => {
    e.stopPropagation();
    setQuestions((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const latest = questions.length > 0 ? questions[0] : null;
  const previousQuestions = questions.slice(1, 4);

  return (
    <div className="flex flex-col gap-2.5" role="region" aria-label="Detected questions">
      {/* Latest question — prominent card */}
      <div
        className={`group flex items-start gap-3 rounded-lg transition-all duration-200 ${
          latest ? "cursor-pointer hover:bg-info/10 question-card-enter" : ""
        }`}
        onClick={() => latest && handleAssist(0)}
        onKeyDown={(e) => { if (latest && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); handleAssist(0); } }}
        role={latest ? "button" : undefined}
        tabIndex={latest ? 0 : undefined}
        aria-label={latest ? `Question: ${latest.text}. ${latest.assisted ? "Answered" : "Click to assist"}` : undefined}
      >
        <div className="relative mt-0.5 shrink-0" aria-hidden="true">
          <HelpCircle className={`h-5 w-5 transition-colors ${latest ? "text-info" : "text-muted-foreground/50"}`} />
          {latest && !latest.assisted && (
            <span className="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-info animate-pulse" />
          )}
          {latest?.assisted && (
            <span className="absolute -top-0.5 -right-0.5 flex h-3 w-3 items-center justify-center rounded-full bg-success">
              <Check className="h-2 w-2 text-white" />
            </span>
          )}
        </div>

        <div className="flex-1 min-w-0">
          {latest ? (
            <p className="text-sm leading-relaxed font-medium text-foreground/90">
              &ldquo;{latest.text}&rdquo;
            </p>
          ) : (
            <p className="text-xs text-muted-foreground/50">
              Listening for questions from the other party
            </p>
          )}
        </div>

        {latest && (
          <div className="flex shrink-0 items-center gap-1">
            <button
              onClick={(e) => { e.stopPropagation(); handleAssist(0); }}
              aria-label={latest.assisted ? "Already answered" : "Get AI assistance for this question"}
              className={`flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-semibold transition-all duration-150 cursor-pointer ${
                latest.assisted
                  ? "bg-success/10 border border-success/20 text-success"
                  : "bg-info/10 border border-info/20 text-info hover:bg-info/20"
              }`}
            >
              {latest.assisted ? (
                <><Check className="h-3.5 w-3.5" aria-hidden="true" />Answered</>
              ) : (
                <><Sparkles className="h-3.5 w-3.5" aria-hidden="true" />Assist</>
              )}
            </button>
            <button
              onClick={(e) => handleDismiss(0, e)}
              aria-label="Dismiss question"
              className="rounded-lg p-1.5 text-muted-foreground/30 opacity-0 transition-all duration-150 hover:bg-destructive/10 hover:text-destructive group-hover:opacity-100 cursor-pointer"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        )}
      </div>

      {/* Previous questions — compact list */}
      {previousQuestions.length > 0 && (
        <div className="flex flex-col gap-1">
          {previousQuestions.map((q, idx) => {
            const realIdx = idx + 1;
            return (
              <div
                key={`q-${realIdx}-${q.timestamp_ms}`}
                className={`group/q flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-left transition-all duration-150 question-card-enter ${
                  q.assisted
                    ? "bg-success/5 border border-success/10"
                    : "bg-card/20 hover:bg-card/40 hover:border-border/20 cursor-pointer"
                }`}
                onClick={() => !q.assisted && handleAssist(realIdx)}
                role="button"
                tabIndex={0}
                onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); handleAssist(realIdx); } }}
                title={q.text}
              >
                <div className="shrink-0">
                  {q.assisted ? (
                    <div className="flex h-4 w-4 items-center justify-center rounded-full bg-success/20">
                      <Check className="h-2.5 w-2.5 text-success" />
                    </div>
                  ) : (
                    <div className="flex h-4 w-4 items-center justify-center rounded-full bg-muted/30">
                      <Clock className="h-2.5 w-2.5 text-muted-foreground/60" />
                    </div>
                  )}
                </div>

                <span className={`flex-1 truncate text-xs leading-snug transition-colors ${
                  q.assisted
                    ? "text-success/70 font-medium"
                    : "text-muted-foreground/60 group-hover/q:text-foreground/80"
                }`}>
                  {q.text}
                </span>

                {!q.assisted && (
                  <Sparkles className="h-3 w-3 shrink-0 text-info/0 group-hover/q:text-info/60 transition-colors" />
                )}
                <button
                  onClick={(e) => handleDismiss(realIdx, e)}
                  aria-label="Dismiss question"
                  className="rounded p-0.5 text-muted-foreground/0 opacity-0 transition-all duration-150 hover:bg-destructive/10 hover:text-destructive group-hover/q:opacity-100 group-hover/q:text-muted-foreground/30 cursor-pointer"
                >
                  <X className="h-3 w-3" />
                </button>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
