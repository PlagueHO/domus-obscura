import { useState } from "react";
import type { JobSummary } from "../src/generated/JobSummary";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import type { ResumePublication } from "../src/generated/ResumePublication";
import { request, type LocalStatus } from "./api";
import type { useCommand } from "./hooks";
import { Button } from "./components/ui";

export function retainedPublicationRun(
  job: JobSummary,
  version: number,
  destinations: PublicationDestination[],
): number | undefined {
  if (
    job.plan_version !== version ||
    !["publishing", "backup_pending"].includes(job.state) ||
    destinations.length === 0
  )
    return undefined;
  const run = destinations[0]?.run_id;
  if (run === undefined || !Number.isSafeInteger(run)) return undefined;
  return run > 0 &&
    destinations.every(
      (destination) =>
        destination.job_id === job.id &&
        destination.plan_version === version &&
        destination.run_id === run,
    )
    ? run
    : undefined;
}

export function PublicationRecovery({
  job,
  version,
  destinations,
  status,
  blocked,
  command,
  onChanged,
}: {
  job: JobSummary;
  version: number;
  destinations: PublicationDestination[];
  status: LocalStatus | undefined;
  blocked: boolean;
  command: ReturnType<typeof useCommand>;
  onChanged: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const run = retainedPublicationRun(job, version, destinations);
  if (run === undefined) return null;
  const disabled = blocked || !status?.enabled || status.busy || command.busy || submitted;
  return (
    <section className="review-section" aria-label="Retained publication recovery">
      <h2>Continue retained publication</h2>
      <p>
        Verify retained copies and continue new-only publication from normalization run {run}. This
        does not download or normalize again. Exact existing files are verified, not overwritten.
        Partial, different or missing receipted files remain errors.
      </p>
      <p className="warning">
        This action can create new files at the frozen destinations. It cannot replace an existing
        album, restore files or remove retained data. The server rechecks the current approval,
        worker lease, staging lock and destination bindings.
      </p>
      {submitted ? (
        <p role="status">
          The server accepted publication recovery. The saved job and receipts show the actual
          result. Refresh the request after the worker finishes before another attempt.
        </p>
      ) : confirming ? (
        <div className="actions" role="group" aria-label="Confirm new-only publication recovery">
          <Button
            disabled={disabled}
            onClick={() =>
              void command.run(async () => {
                const body: ResumePublication = {
                  normalization_run: run,
                  acknowledge_new_only_publication: true,
                };
                await request(`/api/jobs/${job.id}/plans/${version}/publication/resume`, { body });
                setSubmitted(true);
                onChanged();
              })
            }
          >
            {command.busy ? "Sending command…" : "Confirm new-only publication"}
          </Button>
          <Button variant="secondary" disabled={command.busy} onClick={() => setConfirming(false)}>
            Do not continue
          </Button>
        </div>
      ) : (
        <Button variant="secondary" disabled={disabled} onClick={() => setConfirming(true)}>
          Review publication recovery
        </Button>
      )}
    </section>
  );
}
