import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AudioPolicy } from "../src/generated/AudioPolicy";
import { AudioProfileSelect, audioProfileLabel, outputWithAudioPolicy } from "./audio-profiles";
import { audioProfiles, defaultAudioSettings } from "./test-fixtures/audio";

describe("authoritative audio profiles", () => {
  it("uses only the server's accepted profiles and preferred default", () => {
    const { policy } = defaultAudioSettings;
    const markup = renderToStaticMarkup(
      <label>
        Output audio profile
        <AudioProfileSelect
          value={policy.preferred}
          accepted={policy.accepted}
          descriptors={audioProfiles}
          disabled={false}
          onChange={() => {}}
        />
      </label>,
    );
    expect(markup).toContain('value="mp3_cbr320" selected');
    expect(markup).toContain('value="flac_lossless"');
    expect(markup).not.toContain('value="mp3_cbr128"');
    expect(markup).not.toContain('value="mp3_vbr"');
  });

  it("shows each accepted exact profile without merging MP3 bitrates", () => {
    const markup = renderToStaticMarkup(
      <AudioProfileSelect
        value="mp3_cbr192"
        accepted={audioProfiles.map((descriptor) => descriptor.profile)}
        descriptors={audioProfiles}
        disabled
        onChange={() => {}}
      />,
    );
    expect(markup.match(/<option/g)).toHaveLength(8);
    for (const descriptor of audioProfiles) expect(markup).toContain(descriptor.label);
    expect(markup).toContain('value="mp3_cbr192" selected');
    expect(markup).toContain("disabled");
    expect(markup).not.toContain("V0");
    expect(markup).not.toContain("V2");
  });

  it("does not invent a 320 label when descriptors are unavailable", () => {
    expect(audioProfileLabel("mp3_cbr128")).toBe("mp3_cbr128");
    expect(audioProfileLabel("mp3_vbr")).toBe("mp3_vbr");
  });

  it("keeps an invalid preferred choice visibly unselected", () => {
    const markup = renderToStaticMarkup(
      <AudioProfileSelect
        value="mp3_cbr320"
        accepted={["mp3_vbr"]}
        descriptors={audioProfiles}
        disabled={false}
        onChange={() => {}}
      />,
    );
    expect(markup).toContain("Choose an accepted profile");
    expect(markup).not.toContain('value="mp3_cbr320"');
    expect(markup).not.toContain('value="mp3_vbr" selected');
  });

  it.each(audioProfiles)(
    "saves exact policy and compatible codec fields for $profile",
    (choice) => {
      const policy: AudioPolicy = {
        version: 1,
        accepted: audioProfiles.map((descriptor) => descriptor.profile),
        preferred: choice.profile,
      };
      const settings = outputWithAudioPolicy(defaultAudioSettings.settings, policy, audioProfiles);
      expect(settings.audio_policy).toEqual(policy);
      expect(settings.format).toBe(choice.format);
      expect(settings.accepted_formats).toEqual(["mp3", "flac"]);
      expect(defaultAudioSettings.settings.audio_policy).toBeUndefined();
    },
  );

  it("rejects incomplete descriptor data instead of guessing codec fields", () => {
    expect(() =>
      outputWithAudioPolicy(defaultAudioSettings.settings, defaultAudioSettings.policy, []),
    ).toThrow("Reload system settings");
  });
});
