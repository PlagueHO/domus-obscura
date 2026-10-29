import { useState } from "react";
import type { Location } from "../src/generated/Location";
import type { LibraryMatchQuery } from "../src/generated/LibraryMatchQuery";
import type { LibraryMatches } from "../src/generated/LibraryMatches";
import { request } from "./api";
import { useResource } from "./hooks";
import { Alert, Button } from "./components/ui";
import { LibraryCoverageView, libraryLink } from "./LibraryBrowse";

export function LocalMatches({
  locations,
  queries,
}: {
  locations: Location[];
  queries: LibraryMatchQuery[];
}) {
  if (!locations.length || !queries.length) return null;
  return <MatchResults key={JSON.stringify(queries)} locations={locations} queries={queries} />;
}

function MatchResults({
  locations,
  queries,
}: {
  locations: Location[];
  queries: LibraryMatchQuery[];
}) {
  const [selected, setSelected] = useState(locations[0]?.id);
  const location = locations.find((item) => item.id === selected) ?? locations[0];
  const result = useResource(
    (signal) =>
      request<LibraryMatches>(`/api/locations/${location?.id}/library/matches`, {
        body: queries,
        signal,
      }),
    `${location?.id}:${JSON.stringify(queries)}`,
  );
  const matchedLocation = result.data?.coverage.location_id ?? location?.id ?? 0;
  return (
    <section className="review-section" aria-label="Possible local matches">
      <h2>Possible local matches</h2>
      <p>
        Saved artist and album tags only. These matches do not confirm an exact edition or approve a
        download.
      </p>
      <label>
        Match location
        <select
          value={location?.id ?? ""}
          onChange={(event) => setSelected(Number(event.target.value))}
        >
          {locations.map((item) => (
            <option key={item.id} value={item.id}>
              {item.kind} · {item.root}
            </option>
          ))}
        </select>
      </label>
      <Button variant="secondary" disabled={result.loading} onClick={result.refresh}>
        Reload local matches
      </Button>
      <Alert error={result.error} />
      {result.loading && <p role="status">Reading indexed matches…</p>}
      {Boolean(result.error) && (
        <p className="warning">
          Local match information is unavailable or stale. This does not confirm absence.
        </p>
      )}
      {result.data && (
        <>
          <LibraryCoverageView coverage={result.data.coverage} />
          {result.data.matches.map((match) => {
            const query = queries[match.query_index];
            return (
              <div className="list-row" key={match.query_index}>
                <div>
                  <strong>
                    {query?.artist}
                    {query?.album ? ` · ${query.album}` : ""}
                  </strong>
                  {query?.album ? (
                    match.albums.length ? (
                      match.albums.map((album) => (
                        <p key={album.id}>
                          <a
                            href={libraryLink(matchedLocation, { view: "albums", album: album.id })}
                          >
                            Possible tag match · {album.folder || "Root folder"}
                          </a>
                        </p>
                      ))
                    ) : (
                      <p>No indexed album match in this location.</p>
                    )
                  ) : match.artist_id ? (
                    <p>
                      <a
                        href={libraryLink(matchedLocation, {
                          view: "albums",
                          artist: match.artist_id,
                        })}
                      >
                        Possible artist tag match
                      </a>
                    </p>
                  ) : (
                    <p>No indexed artist match in this location.</p>
                  )}
                  {query?.album && match.artist_id && (
                    <p>
                      <a
                        href={libraryLink(matchedLocation, {
                          view: "albums",
                          artist: match.artist_id,
                        })}
                      >
                        Browse all indexed artist albums
                      </a>
                    </p>
                  )}
                </div>
              </div>
            );
          })}
          <p className="muted">
            Up to 50 folder matches per album. Open the indexed artist to browse all saved albums.
            Check main and backup locations separately.
          </p>
        </>
      )}
    </section>
  );
}
