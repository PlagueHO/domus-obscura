import { useState } from "react";
import type { Entry } from "../src/generated/Entry";
import type { Location } from "../src/generated/Location";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Card, Empty, Pager } from "./components/ui";

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

export function Library({ locations }: { locations: Location[] }) {
  const [selected, setSelected] = useState<number>();
  const location = locations.find((item) => item.id === selected) ?? locations[0];
  const [view, setView] = useState<{ root: number | undefined; folder: string; pages: number[] }>({
    root: location?.id,
    folder: "",
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
          onClick={entries.refresh}
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
                setSelected(Number(event.target.value));
                setView({ root: Number(event.target.value), folder: "", pages: [0] });
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
