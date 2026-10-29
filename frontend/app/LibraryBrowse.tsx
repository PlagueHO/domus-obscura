import { useEffect, useState } from "react";
import type { Location } from "../src/generated/Location";
import type { LibraryArtist } from "../src/generated/LibraryArtist";
import type { LibraryAlbum } from "../src/generated/LibraryAlbum";
import type { LibraryFile } from "../src/generated/LibraryFile";
import type { LibraryCoverage } from "../src/generated/LibraryCoverage";
import type { LibraryRefresh } from "../src/generated/LibraryRefresh";
import type { WorkerRequest } from "../src/generated/WorkerRequest";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Empty, Pager } from "./components/ui";

export function libraryLink(location: number, values: Record<string, string | number>) {
  return `#/library?${query({ location, ...values })}`;
}

export function LibraryCoverageView({ coverage }: { coverage: LibraryCoverage }) {
  return (
    <div className="library-evidence">
      <p>
        Saved location coverage: {coverage.coverage} ·{" "}
        {coverage.observed_at === null
          ? "No complete observation time"
          : `Observed ${new Date(coverage.observed_at * 1000).toLocaleString()}`}
      </p>
      <p>
        {coverage.metadata_files} file inspections · {coverage.metadata_issues} metadata issues ·{" "}
        {coverage.unobserved_files} files without current metadata.
      </p>
      <p className="muted">
        Saved observations are not a live availability check. Missing tags, incomplete coverage and
        no indexed match do not prove absence.
      </p>
      {coverage.last_error && <p className="warning">{coverage.last_error}</p>}
    </div>
  );
}

export function FolderRefresh({
  location,
  folder,
  readOnly,
  revision = 0,
}: {
  location: number;
  folder: string;
  readOnly: boolean;
  revision?: number;
}) {
  const command = useCommand();
  const [submitted, setSubmitted] = useState(false);
  const [accepted, setAccepted] = useState<LibraryRefresh>();
  const [poll, setPoll] = useState(false);
  const work = useResource(
    (signal) =>
      accepted?.request
        ? request<WorkerRequest>(`/api/worker/requests/${accepted.request.id}`, { signal })
        : Promise.resolve(undefined),
    String(accepted?.request?.id),
    poll ? 2000 : 0,
  );
  const coverage = useResource(
    (signal) =>
      request<LibraryCoverage>(`/api/locations/${location}/library/coverage?${query({ folder })}`, {
        signal,
      }),
    `${location}:${folder}:${revision}`,
  );
  const state = work.data?.state ?? accepted?.request?.state;
  useEffect(() => {
    if (state && ["complete", "failed", "waiting"].includes(state)) {
      setPoll(false);
      coverage.refresh();
    }
  }, [state, work.data, coverage.refresh]);
  return (
    <section aria-label="Folder scan">
      <Button
        variant="secondary"
        disabled={readOnly || command.busy || poll}
        onClick={() =>
          void command.run(async () => {
            try {
              const result = await request<LibraryRefresh>(
                `/api/locations/${location}/library/refresh`,
                { body: { folder } },
              );
              setAccepted(result);
              setSubmitted(true);
              setPoll(Boolean(result.request));
              work.refresh();
            } finally {
              coverage.refresh();
            }
          })
        }
      >
        Refresh folder from disk
      </Button>
      <p className="muted">
        Scans this folder and its direct entries only. Child folders and media stay unchanged.{" "}
        {readOnly && "System settings are read-only."}
      </p>
      <Alert error={command.error || coverage.error || work.error} />
      {submitted && (
        <p role="status">
          {accepted?.request
            ? `Worker request ${accepted.request.id}: ${state ?? "unknown"}. Queued requests wait while the worker is offline.`
            : "Folder refresh completed."}
        </p>
      )}
      {work.data?.last_error && <p className="warning">{work.data.last_error}</p>}
      {accepted?.request && state === "failed" && (
        <Button
          variant="secondary"
          disabled={readOnly || command.busy || work.loading || Boolean(work.error)}
          onClick={() =>
            void command.run(async () => {
              await request(`/api/worker/requests/${accepted.request?.id}/retry`, { body: {} });
              setPoll(true);
              work.refresh();
            })
          }
        >
          Retry this folder refresh
        </Button>
      )}
      {coverage.data?.folder_state && (
        <p>
          Saved folder state: {coverage.data.folder_state} ·{" "}
          {coverage.data.folder_observed_at === null
            ? "No completed observation"
            : new Date(coverage.data.folder_observed_at * 1000).toLocaleString()}
        </p>
      )}
      {coverage.data?.folder_error && <p className="warning">{coverage.data.folder_error}</p>}
    </section>
  );
}

export function LibraryFileDetails({
  location,
  entry,
  revision = 0,
}: {
  location: number;
  entry: number;
  revision?: number;
}) {
  const file = useResource(
    (signal) =>
      request<LibraryFile>(`/api/locations/${location}/library/files/${entry}`, { signal }),
    `${location}:${entry}:${revision}`,
  );
  const [preview, setPreview] = useState(false);
  const [failed, setFailed] = useState(false);
  const metadata = file.data?.metadata;
  const tags = metadata?.tags;
  const audio = metadata?.audio;
  return (
    <section>
      <Alert error={file.error} />
      <Button variant="secondary" disabled={file.loading} onClick={file.refresh}>
        Reload saved file details
      </Button>
      {file.loading && <p role="status">Reading saved file details…</p>}
      {file.data && (
        <>
          <h2>{file.data.relative_path}</h2>
          {Boolean(file.error) && (
            <p className="warning">Showing an older saved response. It is not current evidence.</p>
          )}
          <p>
            <Badge>{metadata?.status}</Badge> ·{" "}
            {file.data.present
              ? "Present in saved observation"
              : "Not present in the last complete observation"}
          </p>
          <p>
            Observed:{" "}
            {file.data.observed_at === null
              ? "Unknown"
              : new Date(file.data.observed_at * 1000).toLocaleString()}
          </p>
          {metadata?.error && <p className="warning">{metadata.error}</p>}
          <dl className="library-details">
            <dt>Title tag</dt>
            <dd>{tags?.title ?? "Unknown"}</dd>
            <dt>Track artists</dt>
            <dd>{tags?.artists.join("; ") || "Unknown"}</dd>
            <dt>Album artists</dt>
            <dd>{tags?.album_artists.join("; ") || "Unknown"}</dd>
            <dt>Album tag</dt>
            <dd>{tags?.album ?? "Unknown"}</dd>
            <dt>Date</dt>
            <dd>{tags?.date ?? "Unknown"}</dd>
            <dt>Track / total</dt>
            <dd>{tags?.track ?? "Unknown"}</dd>
            <dt>Disc / total</dt>
            <dd>{tags?.disc ?? "Unknown"}</dd>
            <dt>Format</dt>
            <dd>{audio?.format ?? "Not inspected as audio"}</dd>
            <dt>Sample rate</dt>
            <dd>{audio?.sample_rate ? `${audio.sample_rate} Hz` : "Unknown"}</dd>
            <dt>Channels</dt>
            <dd>{audio?.channels ?? "Unknown"}</dd>
            <dt>Bits per sample</dt>
            <dd>{audio?.bits_per_sample ?? "Unknown"}</dd>
            <dt>Header bitrate</dt>
            <dd>
              {audio?.bitrate_kbps
                ? `${audio.bitrate_kbps} kbps; not a whole-file CBR check`
                : "Unknown"}
            </dd>
            <dt>Header duration</dt>
            <dd>{audio?.duration_ms === null ? "Unknown" : `${audio?.duration_ms} ms`}</dd>
            <dt>Embedded cover</dt>
            <dd>{tags?.embedded_cover ? "Observed; not extracted" : "Not observed"}</dd>
            <dt>Size</dt>
            <dd>{metadata?.size === null ? "Unknown" : `${metadata?.size} bytes`}</dd>
          </dl>
          {tags &&
            Object.entries(tags.identifiers).map(([key, values]) => (
              <p key={key}>
                {key}: {values?.join("; ")}
              </p>
            ))}
          <p className="muted">
            Tags and header properties do not prove an exact edition, full audio integrity or
            acquisition eligibility.
          </p>
          {metadata?.status === "asset" && /\.jpe?g$/i.test(file.data.relative_path) && (
            <>
              <Button
                variant="secondary"
                disabled={Boolean(file.error) || file.loading}
                onClick={() => {
                  setPreview(!preview);
                  setFailed(false);
                }}
              >
                {preview ? "Hide JPEG preview" : "Show JPEG preview"}
              </Button>
              {preview && !failed && !file.loading && !file.error && (
                <img
                  key={`${metadata.native_identity}:${metadata.modified_ns}:${metadata.size}`}
                  className="library-preview"
                  src={`/api/locations/${location}/library/files/${entry}/preview`}
                  alt={`Indexed JPEG: ${file.data.relative_path}`}
                  onError={() => setFailed(true)}
                />
              )}
              {failed && (
                <p className="warning">
                  Preview unavailable. The root or file can be offline, changed, invalid or outside
                  image limits. Refresh this folder before trying again.
                </p>
              )}
            </>
          )}
        </>
      )}
    </section>
  );
}

function FilePage({
  location,
  album,
  assets = false,
  revision,
}: {
  location: number;
  album: number;
  assets?: boolean;
  revision: number;
}) {
  const [pages, setPages] = useState([0]);
  const after = pages.at(-1) ?? 0;
  const result = useResource(
    (signal) =>
      request<LibraryFile[]>(
        `/api/locations/${location}/library/albums/${album}/${assets ? "assets" : "files"}?${query({ after, limit: 50 })}`,
        { signal },
      ),
    `${location}:${album}:${assets}:${after}:${revision}`,
  );
  return (
    <section>
      <h2>{assets ? "Folder assets and unsupported files" : "Indexed album files"}</h2>
      <Alert error={result.error} />
      {result.loading && <p role="status">Reading saved files…</p>}
      {Boolean(result.error) && result.data && (
        <p className="warning">Showing an older saved file list.</p>
      )}
      {result.data?.map((file) => (
        <div className="list-row" key={file.id}>
          <a href={libraryLink(location, { view: "file", entry: file.id })}>{file.relative_path}</a>
          <Badge>{file.metadata.status}</Badge>
        </div>
      ))}
      {!result.loading && !result.error && result.data?.length === 0 && (
        <Empty>No indexed files on this page.</Empty>
      )}
      <Pager
        busy={result.loading || Boolean(result.error)}
        previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
        next={
          result.data?.length === 50
            ? () => setPages([...pages, result.data?.at(-1)?.id ?? after])
            : undefined
        }
      />
    </section>
  );
}

export function LibraryBrowser({
  locations,
  location,
  view,
  artist,
  album,
  entry,
  readOnly,
}: {
  locations: Location[];
  location: number;
  view: string;
  artist: number | undefined;
  album: number | undefined;
  entry: number | undefined;
  readOnly: boolean;
}) {
  const [pages, setPages] = useState([0]);
  const [revision, setRevision] = useState(0);
  const after = pages.at(-1) ?? 0;
  const artists = view === "artists";
  const endpoint = artists ? "artists" : "albums";
  const result = useResource(async (signal) => {
    if (album || entry) return [];
    return request<(LibraryArtist | LibraryAlbum)[]>(
      `/api/locations/${location}/library/${endpoint}?${query({ after, limit: 50, ...(artist ? { artist } : {}) })}`,
      { signal },
    );
  }, `${location}:${view}:${artist}:${album}:${entry}:${after}`);
  const selected = useResource(
    (signal) =>
      album
        ? request<LibraryAlbum>(`/api/locations/${location}/library/albums/${album}`, { signal })
        : Promise.resolve(undefined),
    `${location}:${album}`,
  );
  const coverage = useResource(
    (signal) => request<LibraryCoverage>(`/api/locations/${location}/library/coverage`, { signal }),
    String(location),
  );
  return (
    <>
      <header className={`hero ${artist ? "artist-hero" : ""}`}>
        <div>
          <h1>
            {entry
              ? "Library file"
              : (selected.data?.title ??
                (artist ? "Indexed artist albums" : `Library ${artists ? "artists" : "albums"}`))}
          </h1>
          <p>Read-only saved tags. Separate folders do not prove separate or matching editions.</p>
        </div>
        <Button
          variant="secondary"
          disabled={result.loading || selected.loading || coverage.loading}
          onClick={() => {
            result.refresh();
            selected.refresh();
            coverage.refresh();
            setRevision((value) => value + 1);
          }}
        >
          Refresh index view
        </Button>
      </header>
      <label>
        Library root
        <select
          value={location}
          onChange={(event) => {
            window.location.hash = libraryLink(Number(event.target.value), {
              view: artists ? "artists" : "albums",
            }).slice(1);
          }}
        >
          {locations.map((item) => (
            <option key={item.id} value={item.id}>
              {item.kind} · {item.root}
            </option>
          ))}
        </select>
      </label>
      <Alert error={coverage.error} />
      {coverage.data && <LibraryCoverageView coverage={coverage.data} />}
      <Alert error={result.error || selected.error} />
      {Boolean(result.error || selected.error || coverage.error) && (
        <p className="warning">
          Retained responses are older saved evidence. Reload before using them as current
          information.
        </p>
      )}
      {entry ? (
        <LibraryFileDetails
          key={`${location}:${entry}`}
          location={location}
          entry={entry}
          revision={revision}
        />
      ) : album ? (
        selected.data && (
          <>
            <p>
              {selected.data.artist} · {selected.data.folder || "Root folder"} ·{" "}
              {selected.data.file_count} indexed files
            </p>
            <a href={libraryLink(location, { view: "folders", folder: selected.data.folder })}>
              Open indexed folder
            </a>
            <FolderRefresh
              key={`${location}:${selected.data.folder}`}
              location={location}
              folder={selected.data.folder}
              readOnly={readOnly}
              revision={revision}
            />
            <FilePage
              key={`${location}:${album}:files`}
              location={location}
              album={album}
              revision={revision}
            />
            <FilePage
              key={`${location}:${album}:assets`}
              location={location}
              album={album}
              assets
              revision={revision}
            />
          </>
        )
      ) : (
        <>
          {result.loading && <p role="status">Reading indexed {endpoint}…</p>}
          <div className="library-list">
            {result.data?.map((item) => (
              <div className="list-row" key={item.id}>
                <a
                  href={libraryLink(
                    location,
                    "title" in item
                      ? { view: "albums", album: item.id }
                      : { view: "albums", artist: item.id },
                  )}
                >
                  {"title" in item ? item.title : item.name}
                  {"folder" in item && (
                    <small>
                      {item.artist} · {item.folder || "Root folder"}
                    </small>
                  )}
                </a>
                <span>{item.file_count} files</span>
              </div>
            ))}
          </div>
          {!result.loading && !result.error && result.data?.length === 0 && (
            <Empty>
              No indexed {endpoint} on this page. Missing tags or incomplete coverage can hide
              matches; use Folders to inspect files.
            </Empty>
          )}
          <Pager
            busy={result.loading || Boolean(result.error)}
            previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
            next={
              result.data?.length === 50
                ? () => setPages([...pages, result.data?.at(-1)?.id ?? after])
                : undefined
            }
          />
        </>
      )}
    </>
  );
}
