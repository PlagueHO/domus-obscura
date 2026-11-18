import type { ConfigurationSection } from "../src/generated/ConfigurationSection";

export const configurationSections = [
  {
    id: "general",
    title: "General",
    group: "Core application",
    description: "Server address and browser access.",
  },
  {
    id: "identity",
    title: "Identity",
    group: "Core application",
    description: "Microsoft Entra sign-in and administrator access.",
  },
  {
    id: "tls",
    title: "TLS",
    group: "Core application",
    description: "HTTPS certificate settings.",
  },
  {
    id: "storage",
    title: "Storage/cache",
    group: "Core application",
    description: "Local database and provider cache.",
  },
  {
    id: "music_providers",
    title: "Providers",
    group: "Media · Music",
    description: "Services used to find and download music.",
  },
  {
    id: "music_acquisition",
    title: "Acquisition",
    group: "Media · Music",
    description: "Production worker or development download service.",
  },
  {
    id: "music_sharing",
    title: "Sharing",
    group: "Media · Music",
    description: "Selected library folders shared through the separate Soulseek worker.",
  },
] as const satisfies readonly {
  id: ConfigurationSection;
  title: string;
  group: string;
  description: string;
}[];
