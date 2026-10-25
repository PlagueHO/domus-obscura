import { useState } from "react";
import type { Entry } from "../src/generated/Entry";
import type { Location } from "../src/generated/Location";
import { query, request } from "./api";
import { useResource } from "./hooks";
import { Alert, Badge, Button, Empty, Pager } from "./components/ui";

export function Library({ locations }: { locations: Location[] }) {
  const [selected, setSelected] = useState<number>();
  const location = locations.find((item) => item.id === selected) ?? locations[0];
  const [folder, setFolder] = useState("");
  const [pages, setPages] = useState<number[]>([0]);
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
    setFolder(path);
    setPages([0]);
  }
  return (
    <>
      <header className="hero">
        <div>
          <h1>Your Music library</h1>
          <p>Read-only indexed folders. Browsing never moves or changes media.</p>
        </div>
      </header>
      {locations.length === 0 ? (
        <Empty>No configured library roots. Run local setup first.</Empty>
      ) : (
        <>
          <label>
            Library root
            <select
              value={location?.id ?? ""}
              onChange={(event) => {
                setSelected(Number(event.target.value));
                open("");
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
          <nav className="actions" aria-label="Library folders">
            <Button variant="secondary" onClick={() => open("")}>
              Root
            </Button>
            <Button
              variant="secondary"
              disabled={!folder}
              onClick={() => open(folder.slice(0, Math.max(folder.lastIndexOf("/"), 0)))}
            >
              Parent folder
            </Button>
            <span className="identifier">{folder || "Root folder"}</span>
          </nav>
          <Alert error={entries.error} />
          {entries.loading && <p role="status">Reading the library index…</p>}
          <div className="library-list">
            {entries.data?.map((entry) => (
              <div className="list-row" key={entry.id}>
                <div>
                  {entry.kind === "directory" ? (
                    <Button variant="ghost" onClick={() => open(entry.relative_path)}>
                      {entry.relative_path}
                    </Button>
                  ) : (
                    <strong>{entry.relative_path}</strong>
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
          {entries.data?.length === 0 && (
            <Empty>
              No indexed entries in this folder. Incomplete coverage does not confirm that it is
              empty on disk.
            </Empty>
          )}
          <Pager
            busy={entries.loading}
            previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
            next={
              entries.data?.length === 50
                ? () => setPages([...pages, entries.data?.at(-1)?.id ?? after])
                : undefined
            }
          />
        </>
      )}
    </>
  );
}
