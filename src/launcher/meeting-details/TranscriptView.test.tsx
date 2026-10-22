import { Profiler } from "react";
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TranscriptView } from "./TranscriptView";
import { mergeConsecutiveSegments } from "../../lib/mergeSegments";
import { useAudioPlayerStore } from "../../stores/audioPlayerStore";
import type { TranscriptSegment } from "../../lib/types";
import type { TranscriptSearchState } from "../../hooks/useTranscriptSearch";

// ---------------------------------------------------------------------------
// Row render spy
//
// A React Profiler reports commits for a whole subtree, not per row. To count
// which rows actually re-rendered, the real memoised TranscriptRow is wrapped:
// the wrapper keeps the original memo comparison and calls the original render
// function, recording the segment id each time a row renders.
// ---------------------------------------------------------------------------

const rowRenderSpy = vi.hoisted(() => vi.fn<(segmentId: string) => void>());

vi.mock("./TranscriptRow", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./TranscriptRow")>();
  const { memo } = await import("react");
  type Props = Parameters<typeof actual.TranscriptRow>[0];
  const memoized = actual.TranscriptRow as unknown as {
    type: (props: Props) => React.ReactNode;
    compare: ((a: Props, b: Props) => boolean) | null;
  };
  const Spied = memo((props: Props) => {
    rowRenderSpy(props.segment.id);
    return memoized.type(props);
  }, memoized.compare ?? undefined);
  return { ...actual, TranscriptRow: Spied };
});

// ---------------------------------------------------------------------------
// Layout stubs
//
// jsdom has no layout engine: every clientHeight/offsetHeight is 0, and there
// is no ResizeObserver. @tanstack/react-virtual reads the scroll element's
// viewport from clientWidth/clientHeight and measures rows via offsetHeight
// (no ResizeObserver -> it measures once on mount). We give every element a
// 600 px tall viewport and every transcript row a height of 48 px, so the
// virtualizer computes a real window instead of mounting everything.
// ---------------------------------------------------------------------------

const VIEWPORT_HEIGHT = 600;
const ROW_HEIGHT = 48;

const originalOffsetHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetHeight");

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get: () => VIEWPORT_HEIGHT,
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get: () => 800,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.dataset.testid === "transcript-row" ? ROW_HEIGHT : VIEWPORT_HEIGHT;
    },
  });
  useAudioPlayerStore.setState({ activeSegmentId: null, isPlaying: false });
});

afterEach(() => {
  // clientHeight/clientWidth live on Element.prototype; deleting the shadow restores them.
  delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight;
  delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientWidth;
  if (originalOffsetHeight) {
    Object.defineProperty(HTMLElement.prototype, "offsetHeight", originalOffsetHeight);
  }
});

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const SEGMENT_COUNT = 605;
const MEETING_START = 1_700_000_000_000;

// Alternating speakers so no two consecutive segments merge: 605 raw -> 605 merged.
function makeSegments(count: number): TranscriptSegment[] {
  return Array.from({ length: count }, (_, i) => ({
    id: `seg-${i}`,
    text: `Segment number ${i} with some words in it.`,
    speaker: i % 2 === 0 ? "User" : "Them",
    timestamp_ms: MEETING_START + i * 5000,
    is_final: true,
    confidence: 1,
  }));
}

const search: TranscriptSearchState = {
  query: "",
  setQuery: () => {},
  isOpen: false,
  open: () => {},
  close: () => {},
  matches: [],
  currentMatchIndex: 0,
  nextMatch: () => {},
  prevMatch: () => {},
  totalMatches: 0,
};

const segments = makeSegments(SEGMENT_COUNT);
const mergedSegments = mergeConsecutiveSegments(segments);

function renderTranscript(onRender: React.ProfilerOnRenderCallback = () => {}) {
  return render(
    <Profiler id="transcript" onRender={onRender}>
      <div style={{ height: VIEWPORT_HEIGHT }}>
        <TranscriptView
          segments={segments}
          mergedSegments={mergedSegments}
          search={search}
          meetingStartTime={MEETING_START}
        />
      </div>
    </Profiler>
  );
}

describe("TranscriptView virtualization", () => {
  it("mounts only a window of rows for a 605-segment transcript", () => {
    expect(mergedSegments).toHaveLength(SEGMENT_COUNT);
    renderTranscript();
    const rows = screen.getAllByTestId("transcript-row");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.length).toBeLessThan(60);
  });

  it("re-renders only the two affected rows when the active segment changes", () => {
    const onRender = vi.fn();
    renderTranscript(onRender);

    act(() => useAudioPlayerStore.setState({ activeSegmentId: "seg-2" }));

    onRender.mockClear();
    rowRenderSpy.mockClear();

    act(() => useAudioPlayerStore.setState({ activeSegmentId: "seg-5" }));

    // The change committed...
    expect(onRender).toHaveBeenCalled();
    // ...but only the previously active and newly active rows rendered.
    const rendered = rowRenderSpy.mock.calls.map(([id]) => id).sort();
    expect(rendered).toEqual(["seg-2", "seg-5"]);
    expect(screen.getAllByTestId("transcript-row").find((r) => r.dataset.active === "true")?.dataset.segmentId).toBe("seg-5");
  });
});

describe("TranscriptView remote speakers", () => {
  it("labels tagged lines by person, renamed or Speaker N", () => {
    const tagged: TranscriptSegment[] = [
      { id: "t1", text: "We ship Friday", speaker: "Them", speaker_id: "them", speaker_tag: "S1", timestamp_ms: MEETING_START, is_final: true, confidence: 1 },
      { id: "t2", text: "Not sure", speaker: "Them", speaker_id: "them", speaker_tag: "S2", timestamp_ms: MEETING_START + 10_000, is_final: true, confidence: 1 },
      { id: "t3", text: "Plain", speaker: "Them", speaker_id: "them", timestamp_ms: MEETING_START + 20_000, is_final: true, confidence: 1 },
    ];
    const onRenameSpeaker = vi.fn();
    render(
      <div style={{ height: VIEWPORT_HEIGHT }}>
        <TranscriptView
          segments={tagged}
          mergedSegments={mergeConsecutiveSegments(tagged)}
          search={search}
          meetingStartTime={MEETING_START}
          speakers={[
            { id: "them:S1", display_name: "Erin", source: "remote_tag", stats: { segment_count: 0, word_count: 0, talk_time_ms: 0, last_spoke_ms: 0 } },
          ]}
          onRenameSpeaker={onRenameSpeaker}
        />
      </div>
    );
    expect(screen.getByRole("button", { name: "Erin" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Speaker 2" })).toBeTruthy();
    expect(screen.getByText("Them")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Them" })).toBeNull();
  });
});
