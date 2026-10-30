import { useState } from "react";
import type { AlbumReplacementRestoreDecision } from "../src/generated/AlbumReplacementRestoreDecision";
import type { AlbumRestorePreview } from "../src/generated/AlbumRestorePreview";
import type { JobSummary } from "../src/generated/JobSummary";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import type { ResumePublication } from "../src/generated/ResumePublication";
import type { RecoveryEntry } from "../src/generated/RecoveryEntry";
import { request, type LocalStatus } from "./api";
import { useResource, type useCommand } from "./hooks";
import { Alert, Button } from "./components/ui";

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
  if (job.plan_version !== version || job.state !== "complete") return null;
  const replacements = destinations.filter(
    (destination) =>
      destination.job_id === job.id &&
      destination.plan_version === version &&
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
        Review the retained and current files before you approve each destination. Current files are
        kept after restore unless explicitly deleted in Recovery. Destinations complete separately,
        not as one atomic change across drives.
      </p>
      <p>
        <a href={`#/recovery?job=${job.id}`}>Inspect retained versions and deletion status</a>.
        Saved publication evidence does not prove that a retained version is still available.
        Restore requires a fresh verified preview.
      </p>
      {replacements.map((destination) => (
        <RetainedRestoreAvailability
          key={`${job.id}:${version}:${destination.id}:${destination.replacement?.state}`}
          destination={destination}
          blocked={blocked}
          command={command}
          onChanged={onChanged}
        />
      ))}
    </section>
  );
}

function RetainedRestoreAvailability({
  destination,
  blocked,
  command,
  onChanged,
}: {
  destination: PublicationDestination;
  blocked: boolean;
  command: ReturnType<typeof useCommand>;
  onChanged: () => void;
}) {
  const inventory = useResource(
    (signal) =>
      request<RecoveryEntry[]>(
        `/api/recovery?after=0&limit=1&job=${destination.job_id}&category=retained&location=${destination.location_id}`,
        { signal },
      ),
    `restore-inventory:${destination.job_id}:${destination.plan_version}:${destination.location_id}`,
  );
  const rows = inventory.loading || inventory.error ? undefined : inventory.data;
  return (
    <>
      <Alert error={inventory.error} />
      {inventory.loading && <p role="status">Checking retained version availability…</p>}
      <RestoreDestination
        destination={destination}
        retainedAvailable={Boolean(
          rows?.some(
            (entry) =>
              entry.job_id === destination.job_id &&
              entry.plan_version === destination.plan_version &&
              entry.category === "retained" &&
              entry.location_id === destination.location_id &&
              entry.folder === destination.replacement?.retained_folder &&
              entry.operation_id === null &&
              !["deleted", "deleting", "complete"].includes(entry.state),
          ),
        )}
        blocked={blocked}
        command={command}
        onChanged={onChanged}
      />
    </>
  );
}

export function restorePreviewMatches(
  preview: AlbumRestorePreview,
  destination: PublicationDestination,
) {
  return (
    preview.verified &&
    preview.preview_sha256 !== null &&
    preview.job_id === destination.job_id &&
    preview.plan_version === destination.plan_version &&
    preview.location_id === destination.location_id &&
    preview.destination_id === destination.id &&
    preview.review_id === destination.replacement?.review_id &&
    preview.root_identity === destination.root_identity &&
    preview.root === destination.root &&
    preview.original_folder === destination.replacement?.original_folder &&
    preview.retained_folder === destination.replacement?.retained_folder
  );
}

function RestoreDestination({
  destination,
  retainedAvailable,
  blocked,
  command,
  onChanged,
}: {
  destination: PublicationDestination;
  retainedAvailable: boolean;
  blocked: boolean;
  command: ReturnType<typeof useCommand>;
  onChanged: () => void;
}) {
  const [reviewing, setReviewing] = useState(false);
  const [revision, setRevision] = useState(0);
  const [submitted, setSubmitted] = useState(false);
  const path = `/api/jobs/${destination.job_id}/plans/${destination.plan_version}/publication/replacement/${destination.location_id}/restore`;
  const resource = useResource(
    (signal) =>
      reviewing ? request<AlbumRestorePreview>(path, { signal }) : Promise.resolve(undefined),
    `${path}:${reviewing}:${revision}`,
  );
  const replacement = destination.replacement;
  if (!replacement) return null;
  const preview = reviewing ? resource.data : undefined;
  const status = preview?.status ?? replacement.restore_status;
  const disabled = blocked || command.busy || submitted || !retainedAvailable;
  const verified =
    preview !== undefined &&
    !resource.loading &&
    !resource.error &&
    restorePreviewMatches(preview, destination);
  return (
    <article className="list-row">
      <div>
        <strong>
          {destination.kind} · {destination.root} · {replacement.original_folder}
        </strong>
        {status?.last_error && <p className="warning">{status.last_error}</p>}
        {status?.question_id && (
          <p>
            <a href={`#/inbox/${status.question_id}`}>Open restore recovery question</a>
          </p>
        )}
        {replacement.state === "restored" && !status?.last_error ? (
          <p role="status">
            The reviewed original album was restored. Check Recovery for the displaced version and
            any later deletion. The publication receipts below describe the earlier publication, not
            the restored files.
          </p>
        ) : (
          <>
            <details>
              <summary>Saved replacement evidence</summary>
              <p>{replacement.original_files.map((file) => file.name).join(", ")}</p>
              <p className="identifier">Published manifest · {destination.manifest_sha256}</p>
              <p>This saved evidence is not a fresh restore preview.</p>
            </details>
            {replacement.state !== "published" && replacement.state !== "restored" && (
              <p className="warning">
                A restore was interrupted at {replacement.state}. Continuation checks all saved
                folders against the fixed approval. Changed files cannot be approved here.
              </p>
            )}
            {!retainedAvailable ? (
              <p className="warning">
                Restore is unavailable: retained data may be deleted, marked for deletion or not
                confirmed by the current recovery inventory. Open Recovery to inspect its status.
              </p>
            ) : submitted ? (
              <p role="status">Restore completed. Reload the request to read its saved result.</p>
            ) : (
              <>
                <Button
                  variant="secondary"
                  disabled={disabled}
                  onClick={() => {
                    setReviewing(true);
                    setRevision((value) => value + 1);
                  }}
                >
                  {reviewing ? "Refresh restore preview" : "Review restore"}
                </Button>
                {reviewing && resource.loading && (
                  <p role="status">Verifying retained and current files…</p>
                )}
                {reviewing && <Alert error={resource.error} />}
                {preview && !resource.loading && !resource.error && (
                  <AlbumRestoreReview
                    preview={preview}
                    blocked={disabled || !verified}
                    onConfirm={() =>
                      void command.run(async () => {
                        if (!verified || !destination.album_identity) return;
                        const body: AlbumReplacementRestoreDecision = {
                          review_id: replacement.review_id,
                          expected_manifest_sha256: destination.manifest_sha256,
                          expected_album_identity: destination.album_identity,
                          expected_preview_sha256: preview.preview_sha256,
                        };
                        try {
                          await request(path, { body });
                          setSubmitted(true);
                        } finally {
                          setReviewing(false);
                          onChanged();
                        }
                      })
                    }
                  />
                )}
              </>
            )}
          </>
        )}
      </div>
    </article>
  );
}

export function AlbumRestoreReview({
  preview,
  blocked,
  onConfirm,
}: {
  preview: AlbumRestorePreview;
  blocked: boolean;
  onConfirm: () => void;
}) {
  if (!preview.verified || !preview.preview_sha256) {
    return (
      <p className="warning">
        Restore is blocked: {preview.unavailable_reason ?? "file state could not be verified"}.
        Saved file lists are not current evidence. Make the destination available and review again.
      </p>
    );
  }
  const complete = preview.state === "restored";
  return (
    <div aria-label="Verified restore preview">
      <p>
        Destination · {preview.root} · {preview.original_folder}
      </p>
      <h3>{complete ? "Restored files" : "Retained files to restore"}</h3>
      <ul>
        {preview.retained_files.map((file) => (
          <li key={file.name}>
            {file.name} · {file.bytes.toLocaleString()} bytes
          </li>
        ))}
      </ul>
      <h3>{complete ? "Files kept in recovery" : "Current files to keep in recovery"}</h3>
      {preview.current_files.length ? (
        <ul>
          {preview.current_files.map((file) => (
            <li key={file.name}>
              {file.name} · {file.bytes.toLocaleString()} bytes
            </li>
          ))}
        </ul>
      ) : (
        <p>The verified current folder is empty.</p>
      )}
      <p>Recovery destination · {preview.retained_folder}</p>
      <details>
        <summary>Verified file evidence</summary>
        <p className="identifier">Preview · {preview.preview_sha256}</p>
        {[...preview.retained_files, ...preview.current_files].map((file, index) => (
          <p className="identifier" key={`${index}:${file.name}`}>
            {file.name} · {file.sha256}
          </p>
        ))}
      </details>
      {complete && (
        <p role="status">
          The files match the completed restore. Confirm verification to clear its saved recovery
          question. This does not move files.
        </p>
      )}
      <Button disabled={blocked} onClick={onConfirm}>
        {complete
          ? "Confirm completed restore verification"
          : preview.state === "published"
            ? "Confirm restore"
            : "Continue exact restore"}
      </Button>
    </div>
  );
}
