import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { TransferFile } from "../src/generated/TransferFile";
import type { TransferSummary } from "../src/generated/TransferSummary";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { AudioProfileDescriptor } from "../src/generated/AudioProfileDescriptor";
import { audioProfiles as descriptors } from "./test-fixtures/audio";
import { useResource } from "./hooks";
import { RequestPage, RequestProgress } from "./Request";
import { WorkerRequests } from "./WorkerRequests";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

const savedPlan: StoredPlan = {
  job_id: 3,
  version: 2,
  settings_version: 1,
  approved_at: 1,
  plan: {
    edition: {
      release_id: "exact-release",
      album: "Generated album",
      album_artist: "Generated artist",
      bucket: null,
      tracks: [],
    },
    settings: {
      format: "mp3",
      accepted_formats: null,
      artwork_size: null,
      template: { folder: "album", filename: "track" },
    },
    format: "mp3",
    outputs: [],
    destinations: [],
    accepts_incomplete_coverage: false,
    catalog: null,
  },
};
const savedJob: JobSummary = {
  id: 3,
  plan_version: 2,
  state: "queued",
  created_at: 1,
  album: "Generated album",
  album_artist: "Generated artist",
  approved_at: 1,
  artwork_candidate_id: null,
};

function renderRequest({
  job = savedJob,
  plan = savedPlan,
  error,
  transfers = [],
  summary,
  receiptError,
  receiptLoading = false,
  covers = [],
  audioProfiles,
}: {
  job?: JobSummary;
  plan?: StoredPlan;
  error?: Error;
  transfers?: TransferFile[];
  summary?: TransferSummary;
  receiptError?: Error;
  receiptLoading?: boolean;
  covers?: ArtworkCandidate[];
  audioProfiles?: AudioProfileDescriptor[];
} = {}) {
  for (const [index, data] of [
    job,
    plan,
    [],
    covers,
    { publication: [], transfers, summary },
    [],
    null,
    [],
  ].entries()) {
    vi.mocked(useResource).mockReturnValueOnce({
      data,
      error: index === 4 ? (receiptError ?? error) : error,
      loading: index === 4 && receiptLoading,
      refresh: vi.fn(),
    });
  }
  return renderToStaticMarkup(
    <RequestPage
      id={3}
      version={2}
      status={{ enabled: true, busy: false, last_error: null }}
      onChanged={() => {}}
      audioProfiles={audioProfiles}
    />,
  );
}

beforeEach(() => vi.mocked(useResource).mockReset());

it("links to request-scoped Recovery without inferring completion or retained availability", () => {
  const markup = renderRequest();
  expect(markup).toContain('href="#/recovery?job=3"');
  expect(markup).toContain("separately from album completion");
});

it("shows the frozen exact profile in request and source summaries", () => {
  const markup = renderRequest({
    plan: {
      ...savedPlan,
      plan: { ...savedPlan.plan, audio: { policy_version: 1, profile: "mp3_cbr160" } },
    },
    audioProfiles: descriptors,
  });
  expect(markup).toContain("MP3 · 160 kbps CBR");
  expect(markup).not.toContain("320 kbps");
});

it("shows exact profile identifiers without descriptors and does not relabel legacy plans", () => {
  const markup = renderRequest({
    plan: {
      ...savedPlan,
      plan: { ...savedPlan.plan, audio: { policy_version: 1, profile: "mp3_vbr" } },
    },
  });
  expect(markup).toContain("mp3_vbr");
  expect(markup).not.toContain("320 kbps");
  expect(renderRequest()).not.toContain("320 kbps");
});

describe("production worker request status", () => {
  const worker = {
    online: false,
    stopping: false,
    active_revision: null,
    capacity: 64,
    unfinished: 1,
    active: 0,
  };
  const saved = [
    {
      id: 7,
      state: "waiting",
      last_error: "Resolve the saved decision",
      configuration_binding: "a".repeat(64),
      operation: { operation: "scan", location_id: 1 },
    },
  ];
  it("shows offline admission and explicit recovery without claiming completion", () => {
    vi.mocked(useResource).mockReturnValue({
      data: saved,
      error: undefined,
      loading: false,
      refresh: vi.fn(),
    });
    const markup = renderToStaticMarkup(<WorkerRequests status={worker} />);
    expect(markup).toContain("Worker offline; accepted requests stay queued.");
    expect(markup).toContain("scan · waiting");
    expect(markup).toContain("Retry this saved request");
    expect(markup).toContain("Dismiss worker request");
    expect(markup).toContain("does not cancel the album or delete files");
  });
  it("marks retained data stale and disables recovery after a failed refresh", () => {
    vi.mocked(useResource).mockReturnValue({
      data: saved,
      error: new Error("Refresh failed"),
      loading: false,
      refresh: vi.fn(),
    });
    const markup = renderToStaticMarkup(<WorkerRequests status={{ ...worker, stopping: true }} />);
    expect(markup).toContain("Worker stopping; active work is draining.");
    expect(markup).toContain("Request data is stale");
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Retry this saved request/);
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Dismiss worker request/);
  });
});

describe("request approval progress", () => {
  const summary: TransferSummary = {
    job_id: 3,
    plan_version: 2,
    expected_tracks: 137,
    total: 137,
    received: 137,
    planned: 0,
    submitting: 0,
    active: 0,
    all_received: true,
  };
  const largePlan: StoredPlan = {
    ...savedPlan,
    plan: {
      ...savedPlan.plan,
      edition: {
        ...savedPlan.plan.edition,
        tracks: Array.from({ length: 137 }, () => ({
          artist: "Generated artist",
          title: "Generated track",
          disc: null,
          disc_track: null,
        })),
      },
    },
  };
  const receiptPage: TransferFile[] = Array.from({ length: 50 }, (_, index) => ({
    id: index + 1,
    job_id: 3,
    plan_version: 2,
    track_number: index + 1,
    peer: "Generated peer",
    remote_path: `Generated Album\\${index + 1}.mp3`,
    expected_bytes: 10,
    state: "received",
    remote_token: index + 1,
    received_bytes: 10,
    payload_sha256: Array.from({ length: 32 }, () => 1),
  }));

  it("shows whole-album counts and bounded receipt controls without truncation claims", () => {
    const markup = renderRequest({ plan: largePlan, summary, transfers: receiptPage });
    expect(markup).toContain("Received inputs retained");
    expect(markup).toContain("137 of 137 tracks received");
    expect(markup).toContain("Receipt page 1");
    expect(markup).toContain("Up to 50 rows per page");
    const pager = markup.slice(markup.indexOf('aria-label="Transfer receipt pages"'));
    expect(pager).toMatch(/<button[^>]*disabled[^>]*>Previous page/);
    expect(pager).toMatch(/<button(?![^>]*disabled)[^>]*>Next page/);
    expect(markup).not.toContain("first 100");
  });

  it("does not infer all inputs received from a complete prefix", () => {
    const markup = renderRequest({
      plan: largePlan,
      transfers: receiptPage,
      summary: { ...summary, received: 136, submitting: 1, all_received: false },
    });
    expect(markup).not.toContain("Received inputs retained");
    expect(markup).toContain("Tracks with uncertain submission");
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Start Download/);
  });

  it("allows approved local continuation only from the whole exact-plan summary", () => {
    const cover: ArtworkCandidate = {
      id: 9,
      job_id: 3,
      plan_version: 2,
      source: "local:generated",
      input_sha256: "a".repeat(64),
      background: null,
      profile: 1,
      sha256: "b".repeat(64),
      bytes: 100,
      width: 32,
      height: 32,
      approved_at: 1,
    };
    const markup = renderRequest({
      job: { ...savedJob, state: "downloading" },
      plan: largePlan,
      summary,
      transfers: receiptPage,
      covers: [cover],
    });
    expect(markup).toMatch(/<button(?![^>]*disabled)[^>]*>Continue approved local work/);
    expect(markup).toContain("It does not download again");
    expect(markup).not.toContain("This action connects to the configured source");
    for (const changed of [
      { received: 136, submitting: 1, all_received: false },
      { received: 136, active: 1, all_received: false },
      { job_id: 4 },
      { plan_version: 3 },
    ]) {
      const blocked = renderRequest({
        job: { ...savedJob, state: "downloading" },
        plan: largePlan,
        summary: { ...summary, ...changed },
        transfers: receiptPage,
        covers: [cover],
      });
      expect(blocked).not.toContain("Continue approved local work");
      expect(blocked).toMatch(/<button[^>]*disabled[^>]*>Start Download/);
    }
  });

  it("blocks receipt loading and failed summary reads without hiding retained rows", () => {
    for (const state of [
      { receiptLoading: true },
      { receiptError: new Error("Transfer summary unavailable") },
    ]) {
      const markup = renderRequest({ plan: largePlan, transfers: receiptPage, summary, ...state });
      expect(markup).not.toContain("Received inputs retained");
      expect(markup).toMatch(/<fieldset[^>]*disabled/);
      expect(markup).toContain("Generated Album");
      if (state.receiptError) {
        expect(markup).toContain("retained counts and rows are not a current response");
      } else {
        expect(markup).toContain("Reading saved receipts");
      }
    }
  });
  it("keeps plan, source and exact cover approval separate", () => {
    const markup = renderToStaticMarkup(
      <RequestProgress
        planApproved
        sourceApproved={false}
        coverApproved={false}
        inputsReceived={false}
      />,
    );
    expect(markup).toContain("Separate request approvals");
    expect(markup).toContain("Plan approved");
    expect(markup).toContain("Choose and acknowledge exact source");
    expect(markup).toContain("Choose and approve exact preview");
    expect(markup).not.toContain("Exact JPEG approved");
    expect(markup).not.toContain("<button");
  });

  describe("unified request review", () => {
    it("keeps the actual plan, source, cover and start inside one review with receipts outside", () => {
      const markup = renderRequest();
      const review = markup.slice(markup.indexOf('class="panel request-review"'));
      const end = review.indexOf("</section><section");
      const body = review.slice(0, end);
      expect(body).toContain("Separate request approvals");
      expect(body).toContain("Inspect saved edition and destinations");
      expect(body).toContain("Choose the source");
      expect(body).toContain("Choose the cover");
      expect(body).toContain("Start the approved download");
      expect(body).not.toContain('data-slot="card"');
      expect(review.slice(end)).toContain("Transfer receipts");
      expect(markup).toMatch(/<button[^>]*disabled[^>]*>Start Download/);
    });

    it("requires plan approval before exposing source and cover editors", () => {
      const markup = renderRequest({ plan: { ...savedPlan, approved_at: null } });
      expect(markup).toContain("Approve frozen plan 2");
      expect(markup).toContain("Approve the frozen plan before selecting the source and cover");
      expect(markup).not.toContain("Search Soulseek for this release");
      expect(markup).not.toContain("Find covers for this edition");
    });

    it("blocks stale or failed reads without hiding saved evidence", () => {
      const stale = renderRequest({ job: { ...savedJob, plan_version: 3 } });
      expect(stale).toContain("Open the current plan");
      expect(stale).toMatch(/<fieldset[^>]*disabled/);
      expect(stale).not.toContain("Search Soulseek for this release");
      const failed = renderRequest({ error: new Error("Saved request unavailable") });
      expect(failed).toContain("Saved request unavailable");
      expect(failed).toMatch(/<fieldset[^>]*disabled/);
      expect(failed).toContain("Transfer receipts");
    });

    it("keeps backup-pending and saved recovery distinct from completion", () => {
      const backup = renderRequest({ job: { ...savedJob, state: "backup_pending" } });
      expect(backup).toContain("The backup is not complete");
      expect(backup).not.toContain("server reports this request complete");
      expect(backup).not.toContain("Start the approved download");
      const paused = renderRequest({ job: { ...savedJob, state: "awaiting_input" } });
      expect(paused).toContain("Open the saved decision");
      expect(paused).toContain("Nothing starts automatically");
      expect(paused).not.toContain("Search Soulseek for this release");
    });
  });

  it("does not turn source acknowledgement into cover approval", () => {
    const markup = renderToStaticMarkup(
      <RequestProgress planApproved sourceApproved coverApproved={false} inputsReceived={false} />,
    );
    expect(markup).toContain("Acknowledgement saved");
    expect(markup).not.toContain("Exact JPEG approved");
  });

  it("identifies retained inputs without inventing a legacy source approval", () => {
    const markup = renderToStaticMarkup(
      <RequestProgress planApproved sourceApproved={false} coverApproved inputsReceived />,
    );
    expect(markup).toContain("Received inputs retained");
    expect(markup).toContain("Exact JPEG approved");
    expect(markup).not.toContain("Acknowledgement saved");
    expect(markup).not.toContain("Complete");
  });
});
