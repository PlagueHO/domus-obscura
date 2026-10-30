import { useState } from "react";
import type { AlbumReplacementRestoreDecision } from "../src/generated/AlbumReplacementRestoreDecision";
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
        Verify retained copies and continue approved publication from normalization run {run}. This
        does not download or normalize again. Exact existing files are verified, not overwritten.
        Partial, different or missing receipted files remain errors.
      </p>
      <p className="warning">
        This action can create new files at the frozen destinations. It cannot change a saved album
        decision, restore files or remove retained data. The server rechecks the current approval,
        worker lease, staging lock and destination bindings.
      </p>
      {submitted ? (
        <p role="status">
          The server accepted publication recovery. The saved job and receipts show the actual
          result. Refresh the request after the worker finishes before another attempt.
        </p>
      ) : confirming ? (
        <div className="actions" role="group" aria-label="Confirm publication recovery">
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
            {command.busy ? "Sending command…" : "Confirm publication recovery"}
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

export function AlbumReplacementRestore({
  job,
  version,
  destinations,
  blocked,
  command,
  onChanged,
}: {
  job: JobSummary;
  version: number;
  destinations: PublicationDestination[];
  blocked: boolean;
  command: ReturnType<typeof useCommand>;
  onChanged: () => void;
}) {
  const [confirming, setConfirming] = useState<number>();
  const [submitted, setSubmitted] = useState<number>();
  if (job.plan_version !== version || job.state !== "complete") return null;
  const replacements = destinations.filter(
    (destination) =>
      destination.replacement &&
      [
        "published",
        "restore_prepared",
        "restore_current_moved",
        "restore_original_visible",
        "restored",
      ].includes(destination.replacement.state),
  );
  if (replacements.length === 0) return null;
  return (
    <section className="review-section" aria-label="Retained album restoration">
      <h2>Retained album versions</h2>
      <p>
        Restore uses the exact saved review and checks the current published album again. The
        displaced album remains in hidden recovery storage after restoration.
      </p>
      {replacements.map((destination) => {
        const replacement = destination.replacement;
        if (!replacement) return null;
        const restoring = replacement.state !== "published" && replacement.state !== "restored";
        const isSubmitted = submitted === destination.location_id;
        return (
          <article className="list-row" key={destination.id}>
            <div>
              <strong>
                {destination.kind} · {destination.root} · {replacement.original_folder}
              </strong>
              {replacement.state === "restored" ? (
                <p role="status">
                  The reviewed original album is restored. The replaced album remains retained.
                </p>
              ) : (
                <>
                  <p>
                    Original files to restore:{" "}
                    {replacement.original_files.map((file) => file.name).join(", ")}
                  </p>
                  <p className="muted">
                    Current album identity · {destination.album_identity ?? "unavailable"}
                    <br />
                    Current manifest · {destination.manifest_sha256}
                  </p>
                  {restoring && (
                    <p className="warning">
                      A restore was interrupted. The server will inspect all three saved folders
                      before it continues.
                    </p>
                  )}
                  {isSubmitted ? (
                    <p role="status">
                      The server accepted the restore command. Refresh to read the saved journal
                      state.
                    </p>
                  ) : confirming === destination.location_id ? (
                    <div className="actions" role="group" aria-label="Confirm album restoration">
                      <Button
                        disabled={blocked || command.busy || !destination.album_identity}
                        onClick={() =>
                          void command.run(async () => {
                            const body: AlbumReplacementRestoreDecision = {
                              review_id: replacement.review_id,
                              expected_manifest_sha256: destination.manifest_sha256,
                              expected_album_identity: destination.album_identity ?? "",
                            };
                            await request(
                              `/api/jobs/${job.id}/plans/${version}/publication/replacement/${destination.location_id}/restore`,
                              { body },
                            );
                            setSubmitted(destination.location_id);
                            onChanged();
                          })
                        }
                      >
                        {command.busy
                          ? "Restoring…"
                          : restoring
                            ? "Continue exact restore"
                            : "Confirm restore"}
                      </Button>
                      <Button
                        variant="secondary"
                        disabled={command.busy}
                        onClick={() => setConfirming(undefined)}
                      >
                        Do not restore
                      </Button>
                    </div>
                  ) : (
                    <Button
                      variant="secondary"
                      disabled={blocked || command.busy || !destination.album_identity}
                      onClick={() => setConfirming(destination.location_id)}
                    >
                      {restoring ? "Review interrupted restore" : "Review restore"}
                    </Button>
                  )}
                </>
              )}
            </div>
          </article>
        );
      })}
    </section>
  );
}
