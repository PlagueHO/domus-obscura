import { useState } from "react";
import type { Location } from "../src/generated/Location";
import type { LibrarySetup } from "../src/generated/LibrarySetup";
import type { RegisterSelectedLibraryFolder } from "../src/generated/RegisterSelectedLibraryFolder";
import { request } from "./api";
import { useCommand } from "./hooks";
import {
  Alert,
  Badge,
  Button,
  CompactTable,
  Empty,
  SettingsRow,
  StatusBadge,
  type Status,
} from "./components/ui";
import { Icon } from "./components/Media";
import { PrimaryLibrary } from "./PrimaryLibrary";

export function needsLibrarySetup(setup: LibrarySetup): boolean {
  return setup.primary_id === null || setup.primary_error !== null || !setup.output_configured;
}

export function libraryRole(location: Location): string {
  return location.kind === "main" ? "Primary library" : "Backup library";
}

export function libraryFolder(location: Location): string {
  const path = location.display_path ?? location.root;
  if (path.startsWith("storage-root:")) return `Configured media folder (${path.slice(13)})`;
  if (path.startsWith("paired-media-root:")) return `Paired media folder (${path.slice(18)})`;
  return path.startsWith("\\\\?\\UNC\\")
    ? `\\\\${path.slice(8)}`
    : path.startsWith("\\\\?\\")
      ? path.slice(4)
      : path;
}

export function indexStatus(location: Location): string {
  switch (location.coverage) {
    case "complete":
      return "Last full scan completed";
    case "scanning":
      return "Scan in progress";
    case "incomplete":
      return "Last scan incomplete";
    default:
      return "Not yet scanned";
  }
}

function coverageStatus(location: Location): Status {
  switch (location.coverage) {
    case "complete":
      return "complete";
    case "scanning":
      return "scanning";
    case "incomplete":
      return "attention";
    default:
      return "pending";
  }
}

function coverageLabel(location: Location): string {
  switch (location.coverage) {
    case "complete":
      return "Scanned";
    case "scanning":
      return "Scanning";
    case "incomplete":
      return "Incomplete";
    default:
      return "Not scanned";
  }
}

export function LibraryFolderList({ locations }: { locations: Location[] }) {
  return (
    <div className="library-folder-list">
      {locations.map((location) => (
        <article className="library-folder" key={location.id}>
          <span className="library-folder-icon" aria-hidden="true">
            <Icon name="folder" />
          </span>
          <div className="library-folder-main">
            <h3>{libraryRole(location)}</h3>
            <p className="library-folder-path" title={location.root}>
              {libraryFolder(location)}
            </p>
            <p className="muted">
              {location.scan_only
                ? "Scan only. Domus never changes files here."
                : location.kind === "main"
                  ? "New albums are added here."
                  : "Checked against the primary during scans."}
            </p>
            {location.last_error && <p className="warning">{location.last_error}</p>}
          </div>
          <div className="library-folder-status">
            {!location.native_identity && <Badge tone="warning">Setup required</Badge>}
            <StatusBadge status={coverageStatus(location)}>{coverageLabel(location)}</StatusBadge>
            <span className="muted">
              {indexStatus(location)}
              {location.observed_at !== null &&
                ` · ${new Date(location.observed_at * 1000).toLocaleString()}`}
            </span>
          </div>
        </article>
      ))}
    </div>
  );
}

export function LibraryFolderTable({ label, locations }: { label: string; locations: Location[] }) {
  return (
    <CompactTable label={label} columns={["Folder", "Status"]}>
      {locations.map((location) => (
        <tr key={location.id}>
          <td>
            <div className="folder-table-location">
              <Icon name="folder" />
              <span className="library-folder-path">{libraryFolder(location)}</span>
            </div>
            {location.last_error && <div className="warning">{location.last_error}</div>}
          </td>
          <td>
            {!location.native_identity && <Badge tone="warning">Setup required</Badge>}
            {location.scan_only && <Badge>Scan only</Badge>}
            <StatusBadge status={coverageStatus(location)}>{coverageLabel(location)}</StatusBadge>
          </td>
        </tr>
      ))}
    </CompactTable>
  );
}

export function LibraryFolderRegistration({
  setup,
  locations,
  disabled,
  onChanged,
  embedded = false,
}: {
  setup: LibrarySetup | undefined;
  locations: Location[];
  disabled: boolean;
  onChanged: () => void;
  embedded?: boolean;
}) {
  const available = setup?.folders.filter((folder) => folder.location_id === null) ?? [];
  const [selected, setSelected] = useState("");
  const [registered, setRegistered] = useState(false);
  const hasPrimary = locations.some((location) => location.kind === "main");
  const command = useCommand();
  const folder = available.find((item) => `${item.paired}:${item.root_id}` === selected);
  const busy = disabled || command.busy;
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (!folder || busy) return;
        void command.run(async () => {
          const body: RegisterSelectedLibraryFolder = {
            root_id: folder.root_id,
            paired: folder.paired,
            kind: hasPrimary ? "backup" : "main",
          };
          await request<Location>("/api/library/folders", { body });
          setRegistered(true);
          onChanged();
        });
      }}
    >
      {!embedded && <h3>{hasPrimary ? "Add backup library" : "Choose your primary library"}</h3>}
      <SettingsRow label="Library folder" hint="Select a folder configured on the host." wide>
        {(control) => (
          <div className="path-field">
            <select
              {...control}
              value={selected}
              disabled={busy || available.length === 0}
              required
              onChange={(event) => {
                setSelected(event.target.value);
                setRegistered(false);
              }}
            >
              <option value="">Select a configured folder</option>
              {available.map((item) => (
                <option
                  key={`${item.paired}:${item.root_id}`}
                  value={`${item.paired}:${item.root_id}`}
                >
                  {item.display_path}
                </option>
              ))}
            </select>
            <Button
              type="submit"
              disabled={busy || !folder || locations.length >= 100}
              width={embedded ? "medium" : undefined}
              icon={embedded ? <Icon name="plus" /> : undefined}
              busy={embedded && command.busy}
            >
              {!embedded && command.busy
                ? "Registering…"
                : hasPrimary
                  ? embedded
                    ? "Register folder"
                    : "Add backup"
                  : "Use as primary"}
            </Button>
          </div>
        )}
      </SettingsRow>
      {!available.length && setup && !embedded && (
        <Empty>No unregistered media folders are configured.</Empty>
      )}
      {registered && <p role="status">Folder registered. No scan was started.</p>}
      <Alert error={command.error} />
      {!embedded && (
        <p className="muted">
          Backups come from the host storage configuration. Stop the API and workers, add the
          folder, then restart.
        </p>
      )}
    </form>
  );
}

export function SetupWizard({
  setup,
  locations,
  readOnly,
  loading,
  error,
  onChanged,
}: {
  setup: LibrarySetup;
  locations: Location[];
  readOnly: boolean;
  loading: boolean;
  error: unknown;
  onChanged: () => void;
}) {
  const command = useCommand();
  const primaryReady = setup.primary_id !== null && setup.primary_error === null;
  const [step, setStep] = useState(primaryReady ? 1 : 0);
  const complete = primaryReady && setup.output_configured;
  const busy = loading || Boolean(error) || command.busy;
  return (
    <section className="setup-wizard" aria-labelledby="setup-title">
      <header className="page-heading">
        <div>
          <h1 id="setup-title">Set up your Music library</h1>
          <p>Connect your primary library, then review the shared output policy.</p>
        </div>
        <Button variant="secondary" disabled={busy} onClick={onChanged}>
          Check setup again
        </Button>
      </header>
      <ol className="step-rail" aria-label="Library setup steps">
        {["Primary library", "Output policy", "Finish setup"].map((label, index) => (
          <li
            key={label}
            data-state={index === step ? "current" : index < step ? "done" : "future"}
          >
            <span>{index + 1}</span>
            {label}
          </li>
        ))}
      </ol>
      <Alert error={error || command.error} />
      {readOnly && (
        <p role="status">
          Setup changes are locked by the operator. You can inspect the configuration, but cannot
          save changes.
        </p>
      )}
      {step === 0 && (
        <div className="setup-step">
          <h2>Primary library</h2>
          <p>
            Choose the music collection the app will index. Registration does not move or change its
            files.
          </p>
          <LibraryFolderList locations={locations.filter((location) => location.kind === "main")} />
          {setup.primary_error && (
            <div className="alert" role="alert">
              <strong>The primary library could not be verified.</strong>
              <p>{setup.primary_error}</p>
              <p>
                Restore access to the saved folder, then check setup again. Do not register a
                replacement to bypass this check.
              </p>
            </div>
          )}
          {!readOnly && (
            <PrimaryLibrary
              hasPrimary={setup.primary_id !== null}
              disabled={busy}
              onChanged={onChanged}
            />
          )}
          {setup.primary_id === null &&
            !readOnly &&
            setup.folders.some((folder) => folder.location_id === null) && (
              <LibraryFolderRegistration
                setup={setup}
                locations={locations}
                disabled={busy}
                onChanged={onChanged}
              />
            )}
          <Button disabled={busy || !primaryReady} onClick={() => setStep(1)}>
            Continue to output policy
          </Button>
        </div>
      )}
      {step === 1 && (
        <div className="setup-step">
          <h2>Shared output policy</h2>
          <p>
            This policy controls new album folders, filenames, audio choices and cover size. It does
            not change existing media.
          </p>
          {setup.output_configured ? (
            <p role="status">An output policy is saved. You can review it in System settings.</p>
          ) : (
            <>
              <p>
                Start with MP3 320 kbps as preferred, MP3 and FLAC accepted, and 400 × 400 px
                covers. You can change these choices later.
              </p>
              <Button
                disabled={busy || readOnly}
                onClick={() =>
                  void command.run(async () => {
                    await request<number>("/api/library/setup/output", { body: {} });
                    onChanged();
                  })
                }
              >
                Save initial output policy
              </Button>
            </>
          )}
          <a href="#/settings?section=music_output">Review output policy</a>
          <div className="actions">
            <Button variant="secondary" onClick={() => setStep(0)}>
              Back to primary library
            </Button>
            <Button disabled={busy || !complete} onClick={() => setStep(2)}>
              Continue to finish
            </Button>
          </div>
        </div>
      )}
      {step === 2 && (
        <div className="setup-step">
          <h2>{complete ? "Required setup is complete" : "Required setup is incomplete"}</h2>
          <p>
            Backups and download providers are optional. No provider account is needed to browse
            your library.
          </p>
          <p>
            Use Library scans to update the saved index. A scan checks the primary library and
            registered backups without changing media.
          </p>
          <div className="actions">
            {complete && (
              <a className="button button-default" href="#/library">
                Open library
              </a>
            )}
            <a className="button button-secondary" href="#/library-scans">
              Open Library scans
            </a>
            <a href="#/settings?section=music_roots">Configure optional backup folders</a>
          </div>
        </div>
      )}
    </section>
  );
}
