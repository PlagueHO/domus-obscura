import type { EditionSelection } from "../../src/generated/EditionSelection";
import type { Entry } from "../../src/generated/Entry";
import type { JobEvent } from "../../src/generated/JobEvent";
import type { JobSummary } from "../../src/generated/JobSummary";
import type { Location } from "../../src/generated/Location";
import type { SettingsVersion } from "../../src/generated/SettingsVersion";
import type { StoredPlan } from "../../src/generated/StoredPlan";

export type Snapshot = {
  job_id: number;
  plan_version: number;
  location: Location;
  settings: SettingsVersion;
  jobs: JobSummary[];
  events: JobEvent[];
  plan: StoredPlan | null;
};

export type AlbumReport = {
  source: string;
  directory: string;
  format: string;
  cover_sha256: string;
  tracks: {
    number: number;
    filename: string;
    title: string;
    artist: string;
    album: string;
    disc: number;
    disc_track: number;
    duration_seconds: number;
    tags_verified: boolean;
    evidence: { md5_verified: boolean | null; audio_sha256: string; sha256: string };
  }[];
};

export const sampleEdition: EditionSelection = {
  release_id: "20000000-0000-4000-8000-000000000001",
  album_artist: "The Example",
  album: "Night: Signals",
  bucket: null,
  tracks: [
    { artist: "The Example", title: "First / Light", disc: 1, disc_track: 1 },
    { artist: "The Example & Guest", title: "A Quiet Signal", disc: 1, disc_track: 2 },
    { artist: "The Example", title: "After Midnight", disc: 2, disc_track: 1 },
  ],
};

export async function request<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: {
      "x-domus-csrf": "1",
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(15000),
  });
  if (!response.ok) {
    const text = await response.text();
    let message = text;
    try {
      const parsed: unknown = JSON.parse(text);
      if (
        parsed !== null &&
        typeof parsed === "object" &&
        "error" in parsed &&
        typeof parsed.error === "string"
      )
        message = parsed.error;
    } catch {
      // Axum rejects malformed requests with a plain-text response.
    }
    throw new Error(message || `Request failed (${response.status}).`);
  }
  return response.json() as Promise<T>;
}

export async function entries(folder: string, after = 0): Promise<Entry[]> {
  return request(`/local/entries?${new URLSearchParams({ folder, after: String(after) })}`);
}
