import { useState } from "react";
import type { Entry } from "../src/generated/Entry";
import type { Location } from "../src/generated/Location";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Card, Empty, Pager } from "./components/ui";
import { FolderRefresh, LibraryBrowser, libraryLink } from "./LibraryBrowse";

export function parentFolder(folder: string): string {
  return folder.slice(0, Math.max(folder.lastIndexOf("/"), 0));
}

export function entryName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function RootAvailability({ id }: { id: number }) {
  const [checked, setChecked] = useState(false);
  const command = useCommand();
  return (
    <section aria-label="Library root availability" className="root-availability">
      <div className="actions">
        <Button
          variant="secondary"
          disabled={command.busy}
          onClick={() =>
            void command.run(async () => {
              setChecked(false);
              await request<void>(`/api/locations/${id}/availability`);
              setChecked(true);
            })
          }
        >
          {command.busy ? "Checking root…" : "Check root availability"}
        </Button>
        <span role="status">
          {command.busy
            ? "Checking the saved root binding…"
            : checked
              ? "The bound root was available on the last check."
              : "Root availability is not confirmed."}
        </span>
      </div>
      <Alert error={command.error} />
      {Boolean(command.error) && (
        <p className="warning">
          The root could not be verified. It can be offline, inaccessible or changed. The saved
          index remains available. This error does not confirm that media is absent.
        </p>
      )}
      <p className="muted">
        This explicit check opens only the configured root. It does not scan folders, update
        coverage or change files. A successful check does not authorize a later write.
      </p>
    </section>
  );
}

export function Library({
  locations,
  route = "/library",
  readOnly = true,
}: {
  locations: Location[];
  route?: string;
  readOnly?: boolean;
}) {
  const params = new URLSearchParams(route.split("?")[1]);
  const parameter = params.get("location");
  const requested = parameter === null ? undefined : Number(parameter);
  const listed = locations.find((item) => item.id === requested);
  const selected = useResource(
    (signal) =>
      requested !== undefined && Number.isSafeInteger(requested) && requested > 0 && !listed
        ? request<Location>(`/api/locations/${requested}`, { signal })
        : Promise.resolve(undefined),
    String(requested),
  );
  const location =
    requested === undefined
      ? locations[0]
      : (listed ?? (selected.data?.id === requested ? selected.data : undefined));
  if (parameter !== null && !location) {
    return (
      <Card title="Selected library root">
        <Alert error={selected.error} />
        {selected.loading ? (
          <p role="status">Reading the selected library root…</p>
        ) : (
          <Empty>
            The selected library root is unavailable. No other location is shown in its place.
          </Empty>
        )}
        <Button variant="secondary" disabled={selected.loading} onClick={selected.refresh}>
          Reload selected library root
        </Button>
        <nav className="actions" aria-label="Available library roots">
          {locations.map((item) => (
            <a key={item.id} href={libraryLink(item.id, { view: "folders" })}>
              {item.kind} · {item.root}
            </a>
          ))}
        </nav>
      </Card>
    );
  }
  const available =
    location && !locations.some((item) => item.id === location.id)
      ? [...locations, location]
      : locations;
  const view = params.get("view") ?? "folders";
  return (
    <>
      <nav className="actions" aria-label="Library views">
        {["artists", "albums", "folders"].map((item) => (
          <a
            key={item}
            aria-current={item === view ? "page" : undefined}
            href={libraryLink(location?.id ?? 0, { view: item })}
          >
            {item[0]?.toUpperCase()}
            {item.slice(1)}
          </a>
        ))}
      </nav>
      {view === "folders" || !location ? (
        <LibraryFolders
          key={route}
          locations={available}
          initialRoot={location?.id}
          initialFolder={params.get("folder") ?? ""}
          readOnly={readOnly}
        />
      ) : (
        <LibraryBrowser
          key={route}
          locations={available}
          location={location.id}
          view={view}
          artist={Number(params.get("artist")) || undefined}
          album={Number(params.get("album")) || undefined}
          entry={Number(params.get("entry")) || undefined}
          readOnly={readOnly}
        />
      )}
    </>
  );
}

function LibraryFolders({
  locations,
  initialRoot,
  initialFolder,
  readOnly,
}: {
  locations: Location[];
  initialRoot: number | undefined;
  initialFolder: string;
  readOnly: boolean;
}) {
  const [selected, setSelected] = useState<number | undefined>(initialRoot);
  const [revision, setRevision] = useState(0);
  const location = locations.find((item) => item.id === selected) ?? locations[0];
  const [view, setView] = useState<{ root: number | undefined; folder: string; pages: number[] }>({
    root: location?.id,
    folder: initialFolder,
    pages: [0],
  });
  const folder = view.root === location?.id ? view.folder : "";
  const pages = view.root === location?.id ? view.pages : [0];
  const after = pages[pages.length - 1] ?? 0;
  const entries = useResource(
    async (signal) =>
      location
        ? request<Entry[]>(
            `/api/locations/${location.id}/entries?${query({ folder, after, limit: 50 })}`,
            { signal },
          )
        : [],
    `${location?.id}:${folder}:${after}`,
  );
  function open(path: string) {
    setView({ root: location?.id, folder: path, pages: [0] });
  }
  function page(next: number[]) {
    setView({ root: location?.id, folder, pages: next });
  }
  const blocked = entries.loading || Boolean(entries.error);
  return (
    <>
      <header className="hero">
        <div>
          <h1>Your Music library</h1>
          <p>Read-only indexed folders. Browsing never moves or changes media.</p>
        </div>
        <Button
          variant="secondary"
          disabled={!location || entries.loading}
          onClick={() => {
            entries.refresh();
            setRevision((value) => value + 1);
          }}
        >
          Refresh index view
        </Button>
      </header>
      {locations.length === 0 ? (
        <Card title="No library roots">
          <Empty>
            Register an existing library root in System settings, then run its explicit scan.
          </Empty>
          <a href="#/settings">Open System settings</a>
        </Card>
      ) : (
        <Card title="Indexed folders">
          <label>
            Library root
            <select
              value={location?.id ?? ""}
              onChange={(event) => {
                const root = Number(event.target.value);
                setSelected(root);
                setView({ root, folder: "", pages: [0] });
                window.location.hash = libraryLink(root, { view: "folders" }).slice(1);
              }}
            >
              {locations.map((location) => (
                <option key={location.id} value={location.id}>
                  {location.kind} · {location.root}
                </option>
              ))}
            </select>
          </label>
          {location && (
            <p className="muted">
              Coverage: {location.coverage} ·{" "}
              {location.observed_at === null
                ? "No confirmed observation"
                : `Observed ${new Date(location.observed_at * 1000).toLocaleString()}`}
            </p>
          )}
          {location?.last_error && <p className="warning">{location.last_error}</p>}
          {location && <RootAvailability key={location.id} id={location.id} />}
          {location && (
            <FolderRefresh
              key={`${location.id}:${folder}`}
              location={location.id}
              folder={folder}
              readOnly={readOnly}
              revision={revision}
            />
          )}
          {location && location.coverage !== "complete" && (
            <p className="warning">
              This index has incomplete or unknown coverage. Missing entries do not prove that files
              are absent. Refresh reads the saved index only; it does not scan the disk.
            </p>
          )}
          <nav className="actions" aria-label="Library folders">
            <Button variant="secondary" onClick={() => open("")}>
              Root
            </Button>
            <Button
              variant="secondary"
              disabled={!folder}
              onClick={() => open(parentFolder(folder))}
            >
              Parent folder
            </Button>
            <span className="identifier">{folder || "Root folder"}</span>
          </nav>
          <Alert error={entries.error} />
          {Boolean(entries.error) && entries.data && (
            <p className="warning">
              Showing the last saved response for this folder. Use Refresh index view to try the
              read again. This list is not a current filesystem check.
            </p>
          )}
          {entries.loading && <p role="status">Reading the library index…</p>}
          <div className="library-list">
            {entries.data?.map((entry) => (
              <div className="list-row" key={entry.id}>
                <div>
                  {entry.kind === "directory" ? (
                    <Button variant="ghost" onClick={() => open(entry.relative_path)}>
                      {entryName(entry.relative_path)}
                    </Button>
                  ) : (
                    <strong>{entryName(entry.relative_path)}</strong>
                  )}
                  {entry.kind === "file" && location && (
                    <a href={libraryLink(location.id, { view: "file", entry: entry.id })}>
                      View saved file details
                    </a>
                  )}
                  <small>
                    {entry.kind}
                    {entry.size !== null ? ` · ${entry.size.toLocaleString()} bytes` : ""}
                  </small>
                </div>
                {!entry.present && <Badge>Not present in last complete observation</Badge>}
              </div>
            ))}
          </div>
          {!entries.loading && !entries.error && entries.data?.length === 0 && (
            <Empty>
              No indexed entries in this folder. Incomplete coverage does not confirm that it is
              empty on disk.
            </Empty>
          )}
          <Pager
            busy={blocked}
            previous={pages.length > 1 ? () => page(pages.slice(0, -1)) : undefined}
            next={
              entries.data?.length === 50
                ? () => page([...pages, entries.data?.at(-1)?.id ?? after])
                : undefined
            }
          />
        </Card>
      )}
    </>
  );
}
