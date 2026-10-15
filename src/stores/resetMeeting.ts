// Resets every meeting-scoped Zustand store.
//
// Zustand stores are module singletons *per webview*, and NexQ runs two windows
// (launcher + overlay). Both windows must call this when a new meeting starts,
// otherwise the overlay keeps rendering the previous meeting's transcript, AI
// responses, speakers, bookmarks, etc.
//
// Anything that is per-meeting (not user config) belongs here. The mock for
// `meetingStore` itself is handled by the callers (they set activeMeeting etc.).

export async function resetMeetingScopedStores(): Promise<void> {
  const [
    { useTranscriptStore },
    { useStreamStore },
    { useDevLogStore },
    { useCallLogStore },
    { useBookmarkStore },
    { useActionItemStore },
    { useTopicSectionStore },
    { useSpeakerStore },
    { useTranslationStore },
    { useAudioPlayerStore },
  ] = await Promise.all([
    import("./transcriptStore"),
    import("./streamStore"),
    import("./devLogStore"),
    import("./callLogStore"),
    import("./bookmarkStore"),
    import("./actionItemStore"),
    import("./topicSectionStore"),
    import("./speakerStore"),
    import("./translationStore"),
    import("./audioPlayerStore"),
  ]);

  // Transcript + live streaming AI response
  useTranscriptStore.getState().clearSegments();
  useStreamStore.getState().clearCurrent();
  useStreamStore.setState({ responseHistory: [], pinnedResponses: [], isStreaming: false });

  // Meeting artifacts / panels
  useDevLogStore.getState().clear();
  useCallLogStore.getState().clearAll();
  useCallLogStore.getState().setOpen(false);
  useBookmarkStore.getState().clearBookmarks();
  useActionItemStore.getState().clearItems();
  useTopicSectionStore.getState().clearSections();

  // Speaker identity + stats
  useSpeakerStore.getState().reset();

  // Per-segment translations + any recording being replayed
  useTranslationStore.getState().clearTranslations();
  useAudioPlayerStore.getState().reset();
}
