import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import { useResource } from "./hooks";
import { RequestPage, RequestProgress } from "./Request";

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
}: { job?: JobSummary; plan?: StoredPlan; error?: Error } = {}) {
  for (const data of [job, plan, [], [], { publication: [], transfers: [] }, null, []]) {
    vi.mocked(useResource).mockReturnValueOnce({
      data,
      error,
      loading: false,
      refresh: vi.fn(),
    });
  }
  return renderToStaticMarkup(
    <RequestPage
      id={3}
      version={2}
      status={{ enabled: true, busy: false, last_error: null }}
      onChanged={() => {}}
    />,
  );
}

beforeEach(() => vi.mocked(useResource).mockReset());

describe("request approval progress", () => {
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
