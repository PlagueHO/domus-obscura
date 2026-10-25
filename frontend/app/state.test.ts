import { describe, expect, it } from "vitest";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { LocalStatus } from "../src/generated/LocalStatus";
import { canStart, stateLabel, visibleJobs } from "./state";

const job: JobSummary = { id: 1, plan_version: 2, state: "queued", created_at: 1 };
const plan: StoredPlan = {
  job_id: 1,
  version: 2,
  settings_version: 1,
  approved_at: 1,
  plan: {
    edition: {
      release_id: "release",
      album_artist: "Artist",
      album: "Album",
      bucket: null,
      tracks: [],
    },
    settings: { format: "mp3", template: { folder: "server", filename: "server" } },
    format: "mp3",
    outputs: [],
    destinations: [],
    accepts_incomplete_coverage: false,
    catalog: null,
  },
};
const source: SourceCandidate = {
  id: 3,
  job_id: 1,
  plan_version: 2,
  plan_sha256: "plan",
  review_sha256: "review",
  created_at: 1,
  approved_at: 1,
  listing: { peer: "peer", folder: "remote", directories: 0, files: [] },
  assessment: {
    release_id: "release",
    peer: "peer",
    folder: "remote",
    format: "mp3",
    tracks: [],
    problems: [],
    complete_mapping: true,
    advertised_quality_matches: true,
    edition_verified: false,
    audio_verified: false,
    requires_user_review: true,
  },
};
const cover: ArtworkCandidate = {
  id: 4,
  job_id: 1,
  plan_version: 2,
  source: "local",
  input_sha256: "input",
  background: null,
  profile: 1,
  sha256: "jpeg",
  bytes: 500,
  width: 100,
  height: 100,
  approved_at: 1,
};
const status: LocalStatus = { enabled: true, busy: false, last_error: null };

describe("download action eligibility", () => {
  it("permits an explicitly approved queued request without claiming verified media", () => {
    expect(source.assessment.edition_verified).toBe(false);
    expect(canStart(job, plan, source, cover, status, true)).toBe(true);
  });
  it("requires each separate acknowledgement", () => {
    expect(canStart(job, plan, source, cover, status, false)).toBe(false);
    expect(canStart(job, { ...plan, approved_at: null }, source, cover, status, true)).toBe(false);
    expect(canStart(job, plan, { ...source, approved_at: null }, cover, status, true)).toBe(false);
    expect(canStart(job, plan, source, { ...cover, approved_at: null }, status, true)).toBe(false);
  });
  it("requires actual host enablement and an idle worker", () => {
    expect(canStart(job, plan, source, cover, undefined, true)).toBe(false);
    expect(canStart(job, plan, source, cover, { ...status, enabled: false }, true)).toBe(false);
    expect(canStart(job, plan, source, cover, { ...status, busy: true }, true)).toBe(false);
  });
  it("rejects missing, stale, different-job and incomplete candidates", () => {
    expect(canStart(job, plan, undefined, cover, status, true)).toBe(false);
    expect(canStart(job, plan, source, undefined, status, true)).toBe(false);
    expect(canStart({ ...job, plan_version: 3 }, plan, source, cover, status, true)).toBe(false);
    expect(canStart(job, { ...plan, job_id: 5 }, source, cover, status, true)).toBe(false);
    expect(canStart(job, plan, { ...source, job_id: 5 }, cover, status, true)).toBe(false);
    expect(canStart(job, plan, source, { ...cover, plan_version: 3 }, status, true)).toBe(false);
    expect(
      canStart(
        job,
        plan,
        { ...source, assessment: { ...source.assessment, complete_mapping: false } },
        cover,
        status,
        true,
      ),
    ).toBe(false);
  });
  it.each(["complete", "cancelled", "failed", "awaiting_input", "downloading", "publishing"])(
    "does not offer a fresh run for %s",
    (state) => {
      expect(canStart({ ...job, state }, plan, source, cover, status, true)).toBe(false);
    },
  );
  it("only offers local continuation when the caller has complete received receipts", () => {
    expect(
      canStart({ ...job, state: "downloading" }, plan, undefined, cover, status, true, true),
    ).toBe(true);
    expect(
      canStart({ ...job, state: "backup_pending" }, plan, undefined, cover, status, true, true),
    ).toBe(true);
    expect(canStart({ ...job, state: "failed" }, plan, undefined, cover, status, true, true)).toBe(
      false,
    );
  });
  it("keeps backup pending distinct and hides terminal cancellation", () => {
    expect(stateLabel("backup_pending")).toBe("Primary ready · backup pending");
    expect(stateLabel("complete")).toBe("Complete");
    expect(visibleJobs([job, { ...job, id: 2, state: "cancelled" }])).toEqual([job]);
  });
});
