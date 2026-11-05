import { describe, expect, it } from "vitest";
import {
  preparedSelectionMatches,
  selectionRequest,
  toggleAlbum,
  type SelectedAlbum,
} from "./AlbumSelection";
import type { CatalogReleaseSummary } from "../src/generated/CatalogReleaseSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import { defaultAudioSettings as audioSettings } from "./test-fixtures/audio";

const release: CatalogReleaseSummary = {
  id: "20000000-0000-4000-8000-000000000001",
  title: "First album",
  artist: "Generated artist",
  date: null,
  country: null,
  disambiguation: null,
  status: null,
  group: null,
};

describe("exact album selection", () => {
  it("uses the exact ID, removes duplicates and enforces 25 selections", () => {
    const first = toggleAlbum([], release);
    expect(toggleAlbum(first, { ...release, title: "Changed label" })).toEqual([]);
    const full = Array.from({ length: 25 }, (_, index) => ({
      release: { ...release, id: String(index) },
    }));
    expect(() => toggleAlbum(full, release)).toThrow("25");
    expect(toggleAlbum(full, full[0]!.release)).toHaveLength(24);
  });

  it("prepares separate version-bound requests without approving or starting albums", () => {
    const items: SelectedAlbum[] = [
      { release, job: { job_id: 1, plan_version: 1 } },
      {
        release: { ...release, id: "other" },
        job: { job_id: 2, plan_version: 3 },
        profile: "flac_lossless",
      },
    ];
    const body = selectionRequest(items, audioSettings, [1, 2], true);
    expect(body.albums.map((item) => item.request.job_id)).toEqual([1, 2]);
    expect(body.albums.map((item) => item.request.expected_version)).toEqual([1, 3]);
    expect(body.albums[1]!.request.audio_profile).toBe("flac_lossless");
    expect(body.albums[0]!.request.accepts_incomplete_coverage).toBe(true);
    expect(JSON.stringify(body)).not.toMatch(/approve|acknowledge|start/);
    expect(() =>
      selectionRequest([{ release, creationUnknown: true }], audioSettings, [1], false),
    ).toThrow();
    expect(() => selectionRequest(items, audioSettings, [], false)).toThrow();
  });

  it("accepts only a complete exact batch response, including after a lost response", () => {
    const items: SelectedAlbum[] = [{ release, job: { job_id: 1, plan_version: 1 } }];
    const plan = {
      job_id: 1,
      version: 2,
      settings_version: 1,
      approved_at: null,
      plan: { edition: { release_id: release.id } },
    } as StoredPlan;
    expect(preparedSelectionMatches(items, [plan])).toBe(true);
    expect(preparedSelectionMatches(items, [])).toBe(false);
    expect(preparedSelectionMatches([], [])).toBe(false);
    expect(preparedSelectionMatches(items, [{ ...plan, version: 3 }])).toBe(false);
    expect(preparedSelectionMatches(items, [{ ...plan, job_id: 2 }])).toBe(false);
    expect(preparedSelectionMatches([...items, ...items], [plan, plan])).toBe(false);
  });
});
