import type { AudioProfileDescriptor } from "../../src/generated/AudioProfileDescriptor";
import type { SettingsVersion } from "../../src/generated/SettingsVersion";

export const audioProfiles: AudioProfileDescriptor[] = [
  { profile: "mp3_cbr128", format: "mp3", label: "MP3 · 128 kbps CBR" },
  { profile: "mp3_cbr160", format: "mp3", label: "MP3 · 160 kbps CBR" },
  { profile: "mp3_cbr192", format: "mp3", label: "MP3 · 192 kbps CBR" },
  { profile: "mp3_cbr224", format: "mp3", label: "MP3 · 224 kbps CBR" },
  { profile: "mp3_cbr256", format: "mp3", label: "MP3 · 256 kbps CBR" },
  { profile: "mp3_cbr320", format: "mp3", label: "MP3 · 320 kbps CBR" },
  { profile: "mp3_vbr", format: "mp3", label: "MP3 · VBR (actual bitrate variation)" },
  { profile: "flac_lossless", format: "flac", label: "FLAC · Lossless (no fixed bitrate)" },
];

export const defaultAudioSettings: SettingsVersion = {
  version: 7,
  policy: {
    version: 1,
    accepted: ["mp3_cbr320", "flac_lossless"],
    preferred: "mp3_cbr320",
  },
  audio_profiles: audioProfiles,
  settings: {
    format: "mp3",
    accepted_formats: ["mp3", "flac"],
    artwork_size: "recommended",
    template: { folder: "<Album Artist>\\<Album>", filename: "<Title>.<ext>" },
  },
};
