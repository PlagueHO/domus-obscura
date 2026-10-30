import type { AudioProfile } from "../src/generated/AudioProfile";
import type { AudioProfileDescriptor } from "../src/generated/AudioProfileDescriptor";
import type { AudioPolicy } from "../src/generated/AudioPolicy";
import type { OutputSettings } from "../src/generated/OutputSettings";

export function audioProfileLabel(
  profile: AudioProfile,
  descriptors: AudioProfileDescriptor[] = [],
): string {
  return descriptors.find((descriptor) => descriptor.profile === profile)?.label ?? profile;
}

/** Keep compatibility fields consistent with the authoritative profile descriptors. */
export function outputWithAudioPolicy(
  settings: OutputSettings,
  policy: AudioPolicy,
  descriptors: AudioProfileDescriptor[],
): OutputSettings {
  const preferred = descriptors.find((descriptor) => descriptor.profile === policy.preferred);
  const accepted = policy.accepted.map((profile) =>
    descriptors.find((descriptor) => descriptor.profile === profile),
  );
  if (!preferred || accepted.some((descriptor) => !descriptor)) {
    throw new Error("Audio profile details are missing. Reload system settings.");
  }
  return {
    ...settings,
    audio_policy: policy,
    format: preferred.format,
    accepted_formats: [...new Set(accepted.map((descriptor) => descriptor!.format))],
  };
}

export function AudioProfileSelect({
  value,
  accepted,
  descriptors,
  disabled,
  onChange,
}: {
  value: AudioProfile | undefined;
  accepted: AudioProfile[];
  descriptors: AudioProfileDescriptor[];
  disabled: boolean;
  onChange: (profile: AudioProfile) => void;
}) {
  return (
    <select
      value={value && accepted.includes(value) ? value : ""}
      disabled={disabled}
      onChange={(event) => {
        const profile = accepted.find((profile) => profile === event.target.value);
        if (profile) onChange(profile);
      }}
    >
      {(!value || !accepted.includes(value)) && (
        <option value="" disabled>
          Choose an accepted profile
        </option>
      )}
      {accepted.map((profile) => (
        <option key={profile} value={profile}>
          {audioProfileLabel(profile, descriptors)}
        </option>
      ))}
    </select>
  );
}
