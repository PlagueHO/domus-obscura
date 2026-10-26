import type { JobSummary } from "../src/generated/JobSummary";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { LocalStatus } from "./api";

export const activeStates = new Set([
  "searching",
  "downloading",
  "validating",
  "normalizing",
  "publishing",
]);

export function canStart(
  job: JobSummary,
  plan: StoredPlan,
  source: SourceCandidate | undefined,
  cover: ArtworkCandidate | undefined,
  status: LocalStatus | undefined,
  acknowledged: boolean,
  hasCommittedSource = false,
) {
  return Boolean(
    status?.enabled &&
    !status.busy &&
    acknowledged &&
    job.id === plan.job_id &&
    job.plan_version === plan.version &&
    plan.approved_at !== null &&
    cover?.approved_at !== null &&
    cover !== undefined &&
    cover.plan_version === plan.version &&
    cover.job_id === job.id &&
    ((source?.approved_at !== null &&
      source !== undefined &&
      source.assessment.complete_mapping &&
      source.plan_version === plan.version &&
      source.job_id === job.id) ||
      hasCommittedSource) &&
    (job.state === "queued" ||
      (hasCommittedSource &&
        ["downloading", "validating", "normalizing", "publishing", "backup_pending"].includes(
          job.state,
        ))),
  );
}

export function stateLabel(state: string) {
  if (state === "complete") return "Complete";
  if (state === "backup_pending") return "Primary ready · backup pending";
  return state.replaceAll("_", " ");
}

export function visibleJobs(jobs: JobSummary[]) {
  return jobs.filter((job) => job.state !== "cancelled");
}
