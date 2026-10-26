import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { JobSummary } from "../../src/generated/JobSummary";
import {
  HomeQueue,
  JobStatus,
  QueueCover,
  QueuePage,
  finishedSelection,
  groupJobs,
  jobPresentation,
} from "./Media";

const job: JobSummary = {
  id: 17,
  plan_version: 3,
  created_at: 1,
  album: "An exact <edition>",
  album_artist: "Generated artist",
  approved_at: 1,
  artwork_candidate_id: null,
  state: "queued",
};

describe("approved shared queue components", () => {
  it("loads only the retained approved candidate from the exact request and plan", () => {
    const markup = renderToStaticMarkup(<QueueCover job={{ ...job, artwork_candidate_id: 29 }} />);
    expect(markup).toContain('src="/api/jobs/17/plans/3/artwork/29/image"');
    expect(markup).toContain('alt="Approved cover for An exact &lt;edition&gt;"');
    expect(markup).toContain('loading="lazy"');
    const missing = renderToStaticMarkup(<QueueCover job={job} />);
    expect(missing).not.toContain("<img");
    expect(missing).toContain("No artwork loaded");
  });

  it("groups attention, active work and finished without treating backup pending as complete", () => {
    const pending = { ...job, id: 1, approved_at: null };
    const paused = { ...job, id: 2, state: "awaiting_input" };
    const backup = { ...job, id: 3, state: "backup_pending" };
    const failed = { ...job, id: 4, state: "failed" };
    const active = { ...job, id: 5, state: "downloading" };
    const finished = { ...job, id: 6, state: "complete" };
    const cancelled = { ...job, id: 7, state: "cancelled" };
    expect(groupJobs([pending, paused, backup, failed, active, finished, cancelled])).toEqual({
      attention: [pending, paused, backup, failed],
      active: [active],
      finished: [finished],
    });
    expect(jobPresentation({ ...job, state: "normalizing" })).toBe("verifying");
    expect(jobPresentation({ ...job, state: "publishing" })).toBe("publishing");
  });

  it("uses the same accessible icon pill for request, queue and notification states", () => {
    const markup = renderToStaticMarkup(<JobStatus job={{ ...job, state: "awaiting_input" }} />);
    expect(markup).toContain('class="status-pill status-attention"');
    expect(markup).toContain("Needs your input");
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).not.toContain('style="');
  });

  it("keeps home collapsed without problems and expands for attention", () => {
    const collapsed = renderToStaticMarkup(<HomeQueue jobs={[job]} onChanged={() => {}} />);
    expect(collapsed).toContain('aria-expanded="false"');
    expect(collapsed).toContain("1 in progress");
    expect(collapsed).not.toContain("queue-row");
    const expanded = renderToStaticMarkup(
      <HomeQueue jobs={[{ ...job, state: "awaiting_input" }]} onChanged={() => {}} />,
    );
    expect(expanded).toContain('aria-expanded="true"');
    expect(expanded).toContain("1 need your input");
    expect(expanded).toContain("An exact &lt;edition&gt;");
    expect(expanded).toContain("Generated artist");
  });

  it("shows only five finished rows until explicitly expanded", () => {
    const jobs = Array.from({ length: 8 }, (_, index) => ({
      ...job,
      id: index + 1,
      album: `Finished ${index + 1}`,
      state: "complete",
    }));
    const markup = renderToStaticMarkup(<QueuePage jobs={jobs} onChanged={() => {}} />);
    expect(markup.match(/class="queue-row"/g)).toHaveLength(5);
    expect(markup).toContain("Show all 8");
    expect(markup).not.toContain("Finished 6");
    expect(markup).not.toContain("Cancel download");
    expect(markup).not.toContain("Retry");
    expect(markup).toContain("Clear finished on this page");
    expect(markup).toContain("Remove request 1 from queue");
    expect(finishedSelection([...jobs, { ...job, state: "backup_pending" }])).toEqual(
      jobs.map((job) => ({ job_id: job.id, plan_version: job.plan_version })),
    );
  });

  it("does not mutate or expose a destructive action before confirmation", () => {
    const markup = renderToStaticMarkup(
      <QueuePage
        jobs={[job]}
        onChanged={() => {
          throw new Error("must not mutate");
        }}
        disabled
      />,
    );
    expect(markup).toContain('aria-label="Cancel request 17"');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).not.toContain(">Cancel download</button>");
    expect(markup).not.toContain('role="alert"');
  });
});
