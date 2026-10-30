import { useState } from "react";
import type { RecoveryEntry } from "../src/generated/RecoveryEntry";
import type { RecoveryPreview } from "../src/generated/RecoveryPreview";
import type { RecoveryOperation } from "../src/generated/RecoveryOperation";
import type { RecoveryDecision } from "../src/generated/RecoveryDecision";
import type { RecoveryContinue } from "../src/generated/RecoveryContinue";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Card, Check, Empty, Pager } from "./components/ui";

export function recoveryJobFilter(route: string): number | undefined {
  const value = new URLSearchParams(route.split("?")[1]).get("job");
  const job = Number(value);
  return value !== null && Number.isSafeInteger(job) && job > 0 ? job : undefined;
}

export function recoveryPreviewMatches(entry: RecoveryEntry, preview: RecoveryPreview) {
  return (
    preview.verified &&
    Boolean(preview.preview_sha256) &&
    preview.entry.id === entry.id &&
    preview.entry.job_id === entry.job_id &&
    preview.entry.plan_version === entry.plan_version &&
    preview.entry.version === entry.version &&
    preview.entry.category === entry.category &&
    preview.entry.location_id === entry.location_id &&
    preview.entry.folder === entry.folder &&
    preview.entry.eligible &&
    entry.eligible &&
    entry.category === "retained" &&
    !["deleted", "complete"].includes(entry.state) &&
    !["deleted", "complete"].includes(preview.entry.state)
  );
}

export function recoveryOperationMatches(entry: RecoveryEntry, operation: RecoveryOperation) {
  return (
    entry.operation_id === operation.id &&
    operation.entry_id === entry.id &&
    operation.job_id === entry.job_id &&
    operation.plan_version === entry.plan_version
  );
}

export async function deleteRecoveryEntry(entry: RecoveryEntry, preview: RecoveryPreview) {
  if (
    entry.operation_id !== null ||
    !recoveryPreviewMatches(entry, preview) ||
    !preview.preview_sha256
  ) {
    throw new Error("Review a fresh verified preview for this retained version first.");
  }
  const body: RecoveryDecision = {
    expected_preview_sha256: preview.preview_sha256,
    acknowledge_irreversible_deletion: true,
  };
  return request<RecoveryOperation>(`/api/recovery/${entry.id}/delete`, { body });
}

export function scratchPreparationRetry(entry: RecoveryEntry) {
  return entry.category === "scratch" && entry.state === "pending" && entry.operation_id === null;
}

export async function continueRecoveryEntry(
  entry: RecoveryEntry,
  operation: RecoveryOperation | null,
) {
  if (!(
    (operation === null && scratchPreparationRetry(entry)) ||
    (operation !== null &&
      recoveryOperationMatches(entry, operation) &&
      operation.state !== "complete" &&
      entry.state !== "complete" &&
      ["scratch", "retained"].includes(entry.category))
  )) {
    throw new Error("Refresh the saved operation for this entry before retrying.");
  }
  const body: RecoveryContinue = { expected_operation_id: operation?.id ?? 0 };
  return request<RecoveryOperation>(
    `/api/recovery/${entry.id}/${entry.category === "scratch" ? "cleanup" : "continue"}`,
    { body },
  );
}

export function Recovery({ job }: { job?: number }) {
  const [pages, setPages] = useState([0]);
  const [selected, setSelected] = useState<number>();
  const [revision, setRevision] = useState(0);
  const [actionError, setActionError] = useState<unknown>();
  const [actionNotice, setActionNotice] = useState(false);
  const command = useCommand();
  const after = pages.at(-1) ?? 0;
  const resource = useResource(
    (signal) =>
      request<RecoveryEntry[]>(
        `/api/recovery?${query({ after, limit: 50, ...(job ? { job } : {}) })}`,
        { signal },
      ),
    `recovery:${job ?? "all"}:${after}:${revision}`,
  );
  const rows = resource.loading || resource.error ? undefined : resource.data;
  const entry = rows?.find((row) => row.id === selected);
  function refresh() {
    setRevision((value) => value + 1);
  }
  function changePage(next: number[]) {
    setSelected(undefined);
    setPages(next);
  }
  return (
    <>
      <header className="page-heading">
        <div>
          <h1>Recovery</h1>
          <p>Inspect retained versions and cleanup records. Library browsing stays read-only.</p>
        </div>
        <Button variant="secondary" disabled={resource.loading || command.busy} onClick={refresh}>
          Refresh recovery
        </Button>
      </header>
      {job && (
        <p>
          Request {job} · <a href="#/recovery">Show all requests</a>
        </p>
      )}
      <p>
        Scratch cleanup runs after all destinations verify. A cleanup failure does not change album
        completion. Failed work needs an explicit retry.
      </p>
      <Alert error={resource.error} />
      <Alert error={actionError} />
      {actionNotice && (
        <p role="status">
          The command returned. Check the refreshed operation for its saved result.
        </p>
      )}
      {resource.loading && <p role="status">Loading recovery records…</p>}
      {rows && (
        <Card title="Recovery inventory">
          <p>Recorded bytes are saved evidence, not a measurement of current disk allocation.</p>
          {!rows.length && <Empty>No recovery records on this page.</Empty>}
          {rows.map((row) => (
            <article className="list-row" key={row.id}>
              <div>
                <strong>
                  Entry {row.id} · {row.category}
                </strong>
                <p className="identifier">Version · {row.version}</p>
                <p>{row.folder}</p>
                <p>
                  {row.recorded_bytes === null
                    ? "Recorded bytes unknown"
                    : `${row.recorded_bytes.toLocaleString()} recorded bytes`}
                  {" · "}
                  {row.location_id === null ? "Private staging" : `Location ${row.location_id}`}
                </p>
                <p>
                  <a href={`#/request/${row.job_id}/${row.plan_version}`}>
                    Request {row.job_id} · Plan {row.plan_version}
                  </a>
                </p>
                <Badge>{row.state}</Badge>{" "}
                {!row.eligible && <Badge>Ineligible for new deletion</Badge>}
                {row.last_error && <p className="warning">{row.last_error}</p>}
              </div>
              <Button
                variant="secondary"
                disabled={command.busy}
                aria-pressed={selected === row.id}
                aria-controls="recovery-selected"
                onClick={() => setSelected(row.id)}
              >
                Inspect entry {row.id}
              </Button>
            </article>
          ))}
          <Pager
            busy={resource.loading || command.busy}
            previous={pages.length > 1 ? () => changePage(pages.slice(0, -1)) : undefined}
            next={rows.length === 50 ? () => changePage([...pages, rows.at(-1)!.id]) : undefined}
          />
        </Card>
      )}
      {entry && (
        <RecoveryDetail
          key={`${entry.id}:${entry.version}:${revision}:${entry.state}:${entry.operation_id}`}
          entry={entry}
          command={command}
          onChanged={(error) => {
            setActionError(error);
            setActionNotice(!error);
            refresh();
          }}
        />
      )}
    </>
  );
}

export function RecoveryDetail({
  entry,
  command,
  onChanged,
}: {
  entry: RecoveryEntry;
  command: ReturnType<typeof useCommand>;
  onChanged: (error?: unknown) => void;
}) {
  const [revision, setRevision] = useState(0);
  const resource = useResource(async (signal) => {
    const [preview, operation] = await Promise.all([
      request<RecoveryPreview>(`/api/recovery/${entry.id}/preview`, { signal }),
      request<RecoveryOperation | null>(`/api/recovery/${entry.id}/operation`, { signal }),
    ]);
    return { preview, operation };
  }, `recovery-detail:${entry.id}:${entry.version}:${revision}`);
  const data = resource.loading || resource.error ? undefined : resource.data;
  function invalidate(error?: unknown) {
    setRevision((value) => value + 1);
    onChanged(error);
  }
  return (
    <div id="recovery-selected" aria-live="polite">
      <Card title={`Inspect entry ${entry.id}`}>
        <p className="identifier">Selected version · {entry.version}</p>
        <Button
          variant="secondary"
          disabled={command.busy || resource.loading}
          onClick={() => setRevision((value) => value + 1)}
        >
          Refresh selected preview
        </Button>
        <Alert error={resource.error} />
        <Alert error={command.error} />
        {resource.loading && <p role="status">Checking this version and its saved operation…</p>}
        {data && (
          <RecoveryReview
            key={revision}
            entry={entry}
            preview={data.preview}
            operation={data.operation}
            busy={command.busy}
            onDelete={() =>
              void command.run(async () => {
                try {
                  await deleteRecoveryEntry(entry, data.preview);
                } catch (error) {
                  invalidate(error);
                  throw error;
                }
                invalidate();
              })
            }
            onContinue={() =>
              void command.run(async () => {
                try {
                  await continueRecoveryEntry(entry, data.operation);
                } catch (error) {
                  invalidate(error);
                  throw error;
                }
                invalidate();
              })
            }
          />
        )}
      </Card>
    </div>
  );
}

export function RecoveryReview({
  entry,
  preview,
  operation,
  busy,
  onDelete,
  onContinue,
}: {
  entry: RecoveryEntry;
  preview: RecoveryPreview;
  operation: RecoveryOperation | null;
  busy: boolean;
  onDelete: () => void;
  onContinue: () => void;
}) {
  const [acknowledged, setAcknowledged] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const matches =
    preview.entry.id === entry.id &&
    preview.entry.version === entry.version &&
    preview.entry.job_id === entry.job_id &&
    preview.entry.plan_version === entry.plan_version &&
    preview.entry.category === entry.category &&
    preview.entry.location_id === entry.location_id &&
    preview.entry.folder === entry.folder;
  if (!matches) {
    return (
      <p className="warning">The selected version changed. Refresh recovery before continuing.</p>
    );
  }
  const savedOperation = operation && recoveryOperationMatches(entry, operation) ? operation : null;
  const canRetry =
    savedOperation &&
    savedOperation.state !== "complete" &&
    entry.state !== "complete" &&
    ["scratch", "retained"].includes(entry.category);
  const canRetryPreparation = operation === null && scratchPreparationRetry(preview.entry);
  const canDelete = entry.operation_id === null && recoveryPreviewMatches(entry, preview);
  if (entry.category === "input") {
    return (
      <p>
        Protected input data is view-only.{" "}
        {entry.recorded_bytes === null
          ? "Recorded bytes unknown."
          : `${entry.recorded_bytes.toLocaleString()} recorded bytes.`}{" "}
        This is not a measurement of current disk allocation.
      </p>
    );
  }
  return (
    <div aria-label={`Recovery preview for entry ${entry.id}`}>
      <p>
        Entry {entry.id} · {entry.category} · <Badge>{preview.entry.state}</Badge>
      </p>
      <p className="identifier">Version · {entry.version}</p>
      <p>{preview.entry.folder}</p>
      {!preview.verified && (
        <p className="warning">
          Preview unavailable: {preview.unavailable_reason ?? "Files could not be verified"}.
          Missing, offline or changed files are not proof that cleanup completed.
        </p>
      )}
      {!preview.entry.eligible && (
        <p className="warning">This entry is ineligible for a new deletion.</p>
      )}
      {preview.entry.last_error && <p className="warning">{preview.entry.last_error}</p>}
      {preview.verified && (
        <>
          <h3>Verified files in this version</h3>
          {preview.files.length ? (
            <ul>
              {preview.files.map((file) => (
                <li key={file.name}>
                  {file.name} · {file.bytes.toLocaleString()} bytes
                </li>
              ))}
            </ul>
          ) : (
            <p>The verified folder is empty.</p>
          )}
          <details>
            <summary>Verified identity and hash evidence</summary>
            <p className="identifier">Root · {preview.root_identity}</p>
            <p className="identifier">Directory · {preview.directory_identity}</p>
            <p className="identifier">Current hash · {preview.current_sha256}</p>
            <p className="identifier">
              Preview · {preview.preview_sha256 ?? "No deletion approval"}
            </p>
            {preview.files.map((file) => (
              <p className="identifier" key={file.name}>
                {file.name} · {file.identity} · {file.sha256}
              </p>
            ))}
          </details>
        </>
      )}
      {entry.category === "pending" && (
        <p>Pending publication data is view-only. No cleanup or deletion is available here.</p>
      )}
      {entry.category === "scratch" && (
        <p>
          Scratch cleanup is separate from album completion. This page can only retry its existing
          operation or an admitted manifest preparation.
        </p>
      )}
      {canRetryPreparation && (
        <section className="review-section" aria-label="Scratch cleanup preparation">
          <h3>Scratch cleanup status</h3>
          <p>
            Cleanup was admitted, but no operation was created. Retry manifest preparation
            explicitly. Cleanup will not retry automatically.
          </p>
          <Button variant="secondary" disabled={busy} onClick={onContinue}>
            Retry scratch cleanup
          </Button>
        </section>
      )}
      {savedOperation && (
        <section className="review-section" aria-label="Saved recovery operation">
          <h3>{entry.category === "scratch" ? "Scratch cleanup status" : "Deletion status"}</h3>
          <p>
            Operation {savedOperation.id} · {savedOperation.kind} ·{" "}
            <Badge>{savedOperation.state}</Badge>
          </p>
          <p>
            {savedOperation.removed_files} of {savedOperation.total_files} files removed.
          </p>
          {savedOperation.last_error && <p className="warning">{savedOperation.last_error}</p>}
          {canRetry && (
            <Button variant="secondary" disabled={busy} onClick={onContinue}>
              {entry.category === "scratch"
                ? "Retry scratch cleanup"
                : "Continue approved deletion"}
            </Button>
          )}
          <p>No operation retries automatically. A lost response requires a fresh status check.</p>
        </section>
      )}
      {entry.operation_id !== null && !savedOperation && (
        <p className="warning">The saved operation does not match this entry. Refresh recovery.</p>
      )}
      {canDelete && (
        <section className="review-section" aria-label="Delete one retained version">
          <h3>Delete this retained version</h3>
          <p className="warning">
            Deletion is irreversible. It removes only entry {entry.id}, version {entry.version}. You
            cannot restore this version after deletion. Current album files are not selected.
          </p>
          {confirming ? (
            <div className="actions" role="group" aria-label="Confirm irreversible deletion">
              <Button variant="destructive" disabled={busy} onClick={onDelete}>
                Permanently delete entry {entry.id}
              </Button>
              <Button variant="secondary" disabled={busy} onClick={() => setConfirming(false)}>
                Keep this version
              </Button>
            </div>
          ) : (
            <>
              <Check checked={acknowledged} disabled={busy} onChange={setAcknowledged}>
                I understand that this retained version will be permanently deleted.
              </Check>
              <Button
                variant="destructive"
                disabled={busy || !acknowledged}
                onClick={() => setConfirming(true)}
              >
                Review irreversible deletion
              </Button>
            </>
          )}
        </section>
      )}
    </div>
  );
}
