import { describe, expect, it } from "vitest";
import {
  assistSpeakerLabel,
  defaultRemoteSpeakerName,
  remoteKeyOf,
  remoteSpeakerColor,
  withSpeakerName,
} from "./speakerLabels";

describe("remote speaker labels", () => {
  it("names tags Speaker N and gives each person one stable color", () => {
    expect(defaultRemoteSpeakerName("S1")).toBe("Speaker 1");
    expect(defaultRemoteSpeakerName("S12")).toBe("Speaker 12");
    expect(remoteSpeakerColor("S1")).toBe(remoteSpeakerColor("S1"));
    expect(remoteSpeakerColor("S1")).not.toBe(remoteSpeakerColor("S2"));
  });

  it("keys only tagged lines from the other side of an online call", () => {
    expect(remoteKeyOf({ speaker: "Them", speaker_id: "them", speaker_tag: "S2" })).toBe("them:S2");
    expect(remoteKeyOf({ speaker: "Them", speaker_tag: "S2" })).toBe("them:S2");
    expect(remoteKeyOf({ speaker: "Them", speaker_id: "them" })).toBeNull();
    // In-person: the diarized speaker_id names the person itself.
    expect(remoteKeyOf({ speaker: "Them", speaker_id: "speaker_1", speaker_tag: "S2" })).toBeNull();
    expect(remoteKeyOf({ speaker: "User", speaker_tag: "S1" })).toBeNull();
  });

  it("labels AI context lines by person and keeps You/Them for the rest", () => {
    const names: Record<string, string> = { "them:S1": "Erin", speaker_0: "Dana" };
    const nameOf = (key: string) => names[key];
    expect(assistSpeakerLabel({ speaker: "Them", speaker_id: "them", speaker_tag: "S1" }, nameOf)).toBe("Erin");
    expect(assistSpeakerLabel({ speaker: "Them", speaker_id: "them", speaker_tag: "S3" }, nameOf)).toBe("Speaker 3");
    expect(assistSpeakerLabel({ speaker: "Them", speaker_id: "speaker_0", speaker_tag: "S1" }, nameOf)).toBe("Dana");
    expect(assistSpeakerLabel({ speaker: "Them", speaker_id: "them" }, nameOf)).toBe("Them");
    expect(assistSpeakerLabel({ speaker: "User", speaker_id: "you" }, nameOf)).toBe("User");
  });

  it("names a remote speaker in a meeting's speaker list", () => {
    const added = withSpeakerName(undefined, "them:S2", "Erin");
    expect(added).toHaveLength(1);
    expect(added[0]).toMatchObject({ id: "them:S2", display_name: "Erin", source: "remote_tag", color: remoteSpeakerColor("S2") });
    const renamed = withSpeakerName(added, "them:S2", "Erin K");
    expect(renamed).toHaveLength(1);
    expect(renamed[0].display_name).toBe("Erin K");
  });
});
