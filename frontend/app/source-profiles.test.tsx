import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { SearchReport } from "./api";
import { SourceReview, SourceSearchResults, sourceSearchPage } from "./SourceReview";
import { audioProfiles } from "./test-fixtures/audio";

const candidate: SourceCandidate = {
  id: 1,
  job_id: 1,
  plan_version: 2,
  plan_sha256: "plan",
  review_sha256: "review",
  created_at: 1,
  approved_at: null,
  listing: { peer: "peer", folder: "remote", directories: 1, files: [] },
  assessment: {
    release_id: "release",
    peer: "peer",
    folder: "remote",
    format: "mp3",
    profile: "mp3_cbr192",
    tracks: [],
    complete_mapping: true,
    advertised_quality_matches: false,
    can_approve: false,
    edition_verified: false,
    audio_verified: false,
    requires_user_review: true,
    problems: [
      {
        issue: "advertised_quality_mismatch",
        track_number: 1,
        remote_path: "remote\\One.mp3",
      },
    ],
  },
  alternatives: [],
};

const report: SearchReport = {
  query: "Generated album",
  peers: [],
  issues: [],
  limit_reached: false,
  audio_downloaded: false,
  edition_verified: false,
  groups: [
    {
      peer: "peer",
      folder: "remote",
      files: [
        {
          file: {
            remote_path: "remote\\One.mp3",
            expected_bytes: 1234,
            extension: "mp3",
            bitrate_kbps: 192,
            duration_seconds: null,
            vbr: null,
            sample_rate: null,
            bit_depth: null,
          },
          matches: [{ profile: "mp3_cbr192", preferred: false, quality: "unknown" }],
        },
      ],
    },
  ],
};

describe("profile source review", () => {
  it("shows alternatives without enabling approval for the frozen plan", () => {
    const alternate: SourceCandidate = {
      ...candidate,
      alternatives: [
        {
          preferred: true,
          assessment: {
            ...candidate.assessment,
            profile: "mp3_vbr",
            problems: [],
            advertised_quality_matches: true,
            can_approve: true,
          },
        },
      ],
    };
    const markup = renderToStaticMarkup(
      <SourceReview
        base="/api/jobs/1/plans/2"
        releaseId="release"
        format="mp3"
        audioProfile="mp3_cbr192"
        audioProfiles={audioProfiles}
        candidates={[alternate]}
        selected={alternate}
        onSelect={() => {}}
        refresh={() => {}}
        localEnabled
      />,
    );
    expect(markup).toContain("Frozen plan profile: MP3 · 192 kbps CBR");
    expect(markup).toContain("MP3 · VBR (actual bitrate variation)");
    expect(markup).toContain("Preferred profile");
    expect(markup).toContain("create a new plan");
    expect(markup).toContain("approve it separately");
    expect(markup).toContain("No fallback or download starts automatically");
    expect(markup.match(/type="checkbox"/g)).toHaveLength(1);
    expect(markup).toMatch(/type="checkbox"[^>]*disabled/);
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Acknowledge this exact source/);
  });

  it("groups peer files and keeps an absent VBR flag visibly unknown", () => {
    const markup = renderToStaticMarkup(
      <SourceSearchResults
        report={report}
        audioProfiles={audioProfiles}
        busy={false}
        onFolder={() => {}}
      />,
    );
    expect(markup).toContain("CBR/VBR unknown");
    expect(markup).toContain("Quality unknown");
    expect(markup).toContain("MP3 · 192 kbps CBR");
    expect(markup).toContain("peer");
    expect(markup).toContain("remote");
    expect(markup.match(/Use this folder/g)).toHaveLength(1);
    expect(markup).toContain("do not prove a complete album");
    expect(markup).not.toContain("CBR advertised");
    expect(markup).not.toContain("320");
  });

  it("uses exact identifiers when profile descriptors are missing", () => {
    const markup = renderToStaticMarkup(
      <SourceSearchResults report={report} busy={false} onFolder={() => {}} />,
    );
    expect(markup).toContain("mp3_cbr192");
    expect(markup).not.toContain("320");
  });

  it("bounds each result page to 30 files across folder boundaries", () => {
    const first = report.groups[0]!;
    const groups = [
      { ...first, files: Array.from({ length: 28 }, () => first.files[0]!) },
      { ...first, folder: "another", files: Array.from({ length: 40 }, () => first.files[0]!) },
    ];
    const page = sourceSearchPage(groups, 0);
    expect(page.map((group) => group.files.length)).toEqual([28, 2]);
    expect(sourceSearchPage(groups, 30).map((group) => group.files.length)).toEqual([30]);
    expect(sourceSearchPage(groups, 60).map((group) => group.files.length)).toEqual([8]);
    expect(sourceSearchPage(groups, 90)).toEqual([]);
  });
});
