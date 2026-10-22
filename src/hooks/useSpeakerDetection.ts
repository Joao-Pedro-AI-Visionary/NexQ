import { useEffect } from "react";
import { onSpeakerDetected } from "../lib/events";
import { useSpeakerStore } from "../stores/speakerStore";
import { useMeetingStore } from "../stores/meetingStore";

/**
 * Listens for `speaker_detected` events from the Rust backend (emitted when
 * Deepgram diarization identifies a new speaker) and adds the speaker to the
 * speaker store.
 *
 * Mount this hook in the OverlayView to enable automatic speaker discovery
 * during in-person meetings with diarization enabled. Online meetings keep
 * the fixed You / Them speakers and tell remote people apart by the lines'
 * speaker tags instead, so the event is ignored there.
 */
export function useSpeakerDetection() {
  const addSpeaker = useSpeakerStore((s) => s.addSpeaker);

  useEffect(() => {
    const unlisten = onSpeakerDetected((payload) => {
      if (useMeetingStore.getState().audioMode !== "in_person") return;
      if (payload.speaker_id) {
        addSpeaker(payload.speaker_id);
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [addSpeaker]);
}
