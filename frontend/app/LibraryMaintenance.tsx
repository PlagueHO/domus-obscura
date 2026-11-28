import { useState } from "react";
import type { Location } from "../src/generated/Location";
import type { LibraryScanPolicy } from "../src/generated/LibraryScanPolicy";
import type { SaveLibraryScanPolicy } from "../src/generated/SaveLibraryScanPolicy";
import type { LibraryComparison } from "../src/generated/LibraryComparison";
import type { WorkerStatus } from "../src/generated/WorkerStatus";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button, Card, Empty, Pager } from "./components/ui";
import { libraryFolder, LibraryFolderList } from "./LibrarySetup";
import { WorkerRequests } from "./WorkerRequests";

export function LibraryMaintenance({
  locations,
  readOnly,
  worker,
  onChanged,
}: {
  locations: Location[];
  readOnly: boolean;
  worker: WorkerStatus | undefined;
  onChanged: () => void;
}) {
  const policy = useResource(
    (signal) => request<LibraryScanPolicy>("/api/library/scan-policy", { signal }),
    "library-scan-policy",
    15000,
  );
  const command = useCommand();
  const [submitted, setSubmitted] = useState(false);
  const [backup, setBackup] = useState(
    locations.find((location) => location.kind === "backup")?.id,
  );
  const primary = locations.find((location) => location.kind === "main");
  const backups = locations.filter((location) => location.kind === "backup");
  const blocked = readOnly || command.busy || !primary;
  return (
    <>
      <header className="page-heading">
        <div>
          <h1>Library scans</h1>
          <p>Update the saved index from your primary library and check every registered backup.</p>
        </div>
        <a href="#/settings?section=music_roots">Manage library folders</a>
      </header>
      <Card title="Scan your library">
        <p>
          A scan reads folders and file metadata. It updates the internal library index and reports
          backup differences. It does not copy, replace or delete media.
        </p>
        <p>
          Start one library scan here. Backup folders are included automatically, not scanned as
          separate user actions.
        </p>
        <LibraryFolderList locations={locations} />
        {!primary && <Empty>Configure a primary library before scanning.</Empty>}
        {readOnly && <p role="status">Scans are locked by the operator's read-only policy.</p>}
        <div className="actions">
          <Button
            disabled={blocked}
            onClick={() =>
              void command.run(async () => {
                setSubmitted(false);
                await request<Location>("/api/library/scan", { body: {} });
                setSubmitted(true);
                policy.refresh();
                onChanged();
              })
            }
          >
            {command.busy ? "Requesting scan…" : "Scan primary library and check backups"}
          </Button>
          <Button
            variant="secondary"
            disabled={command.busy}
            onClick={() => {
              onChanged();
              policy.refresh();
            }}
          >
            Refresh scan status
          </Button>
        </div>
        {submitted && (
          <p role="status">
            Library scan requested. Review the worker result and refresh the index status to confirm
            completion.
          </p>
        )}
        <Alert error={command.error} />
      </Card>
      <Card title="Scan schedule">
        <p>
          Automatic scans use the same primary-library scan and backup checks. Keep the application
          and worker running for scheduled scans.
        </p>
        <Alert error={policy.error} />
        {policy.loading && !policy.data && <p role="status">Loading the scan schedule…</p>}
        {policy.data && (
          <ScanSchedule
            key={policy.data.version}
            policy={policy.data}
            disabled={blocked || policy.loading || Boolean(policy.error) || !worker}
            onChanged={policy.refresh}
          />
        )}
        {!worker && <p>A background worker must be configured before you can save a schedule.</p>}
      </Card>
      <Card title="Backup differences">
        <p>
          Compare the saved observations from complete scans. This checks relative paths, entry
          types and file sizes. Matching results do not prove equal file contents. No backup repair
          starts here.
        </p>
        {backups.length ? (
          <>
            <label>
              Backup library
              <select
                value={backup ?? ""}
                onChange={(event) => setBackup(Number(event.target.value))}
              >
                <option value="" disabled>
                  Select a backup library
                </option>
                {backups.map((location) => (
                  <option value={location.id} key={location.id}>
                    {libraryFolder(location)}
                  </option>
                ))}
              </select>
            </label>
            {backup !== undefined && primary && <BackupComparison key={backup} id={backup} />}
          </>
        ) : (
          <Empty>
            No backup folders are registered. Your primary library can still be scanned.
          </Empty>
        )}
      </Card>
      {worker && <WorkerRequests status={worker} />}
    </>
  );
}

function ScanSchedule({
  policy,
  disabled,
  onChanged,
}: {
  policy: LibraryScanPolicy;
  disabled: boolean;
  onChanged: () => void;
}) {
  const [interval, setInterval] = useState(String(policy.interval_minutes ?? ""));
  const command = useCommand();
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (disabled || command.busy) return;
        void command.run(async () => {
          const body: SaveLibraryScanPolicy = {
            expected_version: policy.version,
            interval_minutes: interval === "" ? null : Number(interval),
          };
          await request<LibraryScanPolicy>("/api/library/scan-policy", { method: "PUT", body });
          onChanged();
        });
      }}
    >
      <label>
        Automatic scan frequency
        <select
          value={interval}
          disabled={disabled || command.busy}
          onChange={(event) => setInterval(event.target.value)}
        >
          <option value="">Off · manual scans only</option>
          <option value="60">Every hour</option>
          <option value="1440">Every 24 hours</option>
          <option value="10080">Every 7 days</option>
          {policy.interval_minutes !== null &&
            ![60, 1440, 10080].includes(policy.interval_minutes) && (
              <option value={policy.interval_minutes}>
                Every {policy.interval_minutes} minutes
              </option>
            )}
        </select>
      </label>
      <p className="muted">
        {policy.next_due === null
          ? "No automatic scan is scheduled."
          : `Next scheduled check: ${new Date(policy.next_due * 1000).toLocaleString()}. Missed runs wait for the worker; they are not replayed in a burst.`}
      </p>
      {policy.last_error && (
        <p className="warning" role="status">
          {policy.last_error}
        </p>
      )}
      <Alert error={command.error} />
      <Button
        type="submit"
        disabled={disabled || command.busy || interval === String(policy.interval_minutes ?? "")}
      >
        {command.busy ? "Saving schedule…" : "Save scan schedule"}
      </Button>
    </form>
  );
}

const differenceLabels: Record<string, string> = {
  missing_in_backup: "Missing from backup",
  only_in_backup: "Only in backup",
  file_size_differs: "File sizes differ",
  entry_type_differs: "File and folder types differ",
};

function BackupComparison({ id }: { id: number }) {
  const [pages, setPages] = useState([""]);
  const after = pages.at(-1) ?? "";
  const comparison = useResource(
    (signal) =>
      request<LibraryComparison>(
        `/api/locations/${id}/library/comparison?${query({ after, limit: 50 })}`,
        { signal },
      ),
    `${id}:${after}`,
  );
  const data = comparison.data;
  const complete = data?.primary.coverage === "complete" && data.backup.coverage === "complete";
  return (
    <div>
      <Button variant="secondary" disabled={comparison.loading} onClick={comparison.refresh}>
        Refresh backup comparison
      </Button>
      <Alert error={comparison.error} />
      {comparison.loading && <p role="status">Reading saved backup observations…</p>}
      {data && !comparison.loading && !comparison.error && (
        <>
          <p className="muted">
            Primary observed:{" "}
            {data.primary.observed_at === null
              ? "not scanned"
              : new Date(data.primary.observed_at * 1000).toLocaleString()}
            . Backup observed:{" "}
            {data.backup.observed_at === null
              ? "not scanned"
              : new Date(data.backup.observed_at * 1000).toLocaleString()}
            .
          </p>
          {!complete ? (
            <p className="warning">
              Comparison is unavailable until both folders have complete scan coverage. Missing or
              offline observations do not prove missing files.
            </p>
          ) : (
            <>
              {data.differences.length === 0 && (
                <p>
                  No path, type or size differences on this page. File contents have not been
                  compared.
                </p>
              )}
              <ul className="difference-list">
                {data.differences.map((item) => (
                  <li key={item.path}>
                    <strong>{differenceLabels[item.difference] ?? item.difference}</strong>
                    <span>{item.path}</span>
                  </li>
                ))}
              </ul>
              <Pager
                previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
                next={
                  data.differences.length === 50
                    ? () => setPages([...pages, data.differences.at(-1)?.path ?? after])
                    : undefined
                }
              />
            </>
          )}
        </>
      )}
    </div>
  );
}
