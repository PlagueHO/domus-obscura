import { useEffect, useState } from "react";
import type { CatalogArtist } from "../src/generated/CatalogArtist";
import type { CatalogPage } from "../src/generated/CatalogPage";
import type { CatalogReleasePage } from "../src/generated/CatalogReleasePage";
import type { CatalogResult } from "../src/generated/CatalogResult";
import { query, request } from "./api";
import { useResource } from "./hooks";
import { AlbumCard, ArtistCard } from "./components/Media";
import { Alert, Button, Check, Empty, Pager } from "./components/ui";

/** Space distinct search operations, not retries. The server's durable provider
 * budget remains authoritative, including any longer pause or backoff. */
export function searchGap(signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(signal.reason);
      return;
    }
    const abort = () => {
      clearTimeout(timer);
      reject(signal.reason);
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", abort);
      resolve();
    }, 1100);
    signal.addEventListener("abort", abort, { once: true });
  });
}

export function nextArtistOffset(page: CatalogPage<CatalogArtist>): number | null {
  const next = page.offset + page.items.length;
  return page.items.length > 0 && next < page.count ? next : null;
}

function SearchAlbums({ term }: { term: string }) {
  const [pages, setPages] = useState([0]);
  const offset = pages.at(-1) ?? 0;
  const [expanded, setExpanded] = useState(false);
  const albums = useResource(async (signal) => {
    await searchGap(signal);
    return request<CatalogResult<CatalogReleasePage>>(
      `/api/discovery/releases?${query({
        query: term,
        offset,
        limit: 12,
        include_singles: expanded,
        include_live: expanded,
        include_compilations: expanded,
        include_unofficial: expanded,
      })}`,
      { signal },
    );
  }, `${term}:${offset}:${expanded}`);
  const page = albums.data?.data;
  const next = page?.next_offset;
  return (
    <section className="search-group" aria-labelledby="album-results-heading">
      <header className="search-group-header">
        <h2 id="album-results-heading">Albums and exact editions</h2>
        <Button variant="ghost" disabled={albums.loading} onClick={albums.refresh}>
          Search albums again
        </Button>
      </header>
      <Check
        checked={expanded}
        onChange={(value) => {
          setExpanded(value);
          setPages([0]);
        }}
      >
        Include singles, live, compilations and unofficial editions
      </Check>
      <Alert error={albums.error} />
      {albums.loading && <p role="status">Searching albums on MusicBrainz…</p>}
      {albums.data && (
        <p className="muted">
          {albums.data.cached ? "Cached provider result" : "Provider result"} · {page?.count}{" "}
          provider rows
        </p>
      )}
      <div className="media-grid">
        {page?.items.map((release) => (
          <AlbumCard key={release.id} release={release} />
        ))}
      </div>
      {page?.items.length === 0 && (
        <Empty>No albums on this page. A filtered page can still have a next page.</Empty>
      )}
      <Pager
        busy={albums.loading}
        previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
        next={next !== null && next !== undefined ? () => setPages([...pages, next]) : undefined}
      />
    </section>
  );
}

function SearchResults({ term }: { term: string }) {
  const [pages, setPages] = useState([0]);
  const offset = pages.at(-1) ?? 0;
  const [albumsReady, setAlbumsReady] = useState(false);
  const artists = useResource(
    (signal) =>
      request<CatalogResult<CatalogPage<CatalogArtist>>>(
        `/api/discovery/artists?${query({ query: term, offset, limit: 6 })}`,
        { signal },
      ),
    `${term}:${offset}`,
  );
  useEffect(() => {
    if (!artists.loading) setAlbumsReady(true);
  }, [artists.loading]);
  const page = artists.data?.data;
  const next = page ? nextArtistOffset(page) : null;
  return (
    <>
      <header className="page-heading">
        <div>
          <h1>Search · {term}</h1>
          <p>Choose an artist or an exact album edition. No download starts from search.</p>
        </div>
      </header>
      <section className="search-group" aria-labelledby="artist-results-heading">
        <header className="search-group-header">
          <h2 id="artist-results-heading">Artists</h2>
          <Button variant="ghost" disabled={artists.loading} onClick={artists.refresh}>
            Search artists again
          </Button>
        </header>
        <Alert error={artists.error} />
        {artists.loading && <p role="status">Searching artists on MusicBrainz…</p>}
        {artists.data && (
          <p className="muted">
            {artists.data.cached ? "Cached provider result" : "Provider result"} · {page?.count}{" "}
            provider rows
          </p>
        )}
        <div className="media-grid artist-grid">
          {page?.items.map((artist) => (
            <ArtistCard key={artist.id} artist={artist} />
          ))}
        </div>
        {page?.items.length === 0 && <Empty>No artists on this page.</Empty>}
        <Pager
          busy={artists.loading}
          previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
          next={next !== null ? () => setPages([...pages, next]) : undefined}
        />
      </section>
      {albumsReady ? (
        <SearchAlbums term={term} />
      ) : (
        <section className="search-group">
          <h2>Albums and exact editions</h2>
          <p role="status">Album search follows the first artist request.</p>
        </section>
      )}
    </>
  );
}

export function Search({ term }: { term: string }) {
  if (!term.trim()) return <Empty>Enter an artist or album in the search field.</Empty>;
  return <SearchResults key={term} term={term} />;
}
