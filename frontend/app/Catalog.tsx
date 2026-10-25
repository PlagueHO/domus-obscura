import { useState } from "react";
import type { CatalogArtist } from "../src/generated/CatalogArtist";
import type { CatalogPage } from "../src/generated/CatalogPage";
import type { CatalogRelease } from "../src/generated/CatalogRelease";
import type { CatalogReleasePage } from "../src/generated/CatalogReleasePage";
import type { CatalogReleaseSummary } from "../src/generated/CatalogReleaseSummary";
import type { CatalogResult } from "../src/generated/CatalogResult";
import type { CatalogPlanRequest } from "../src/generated/CatalogPlanRequest";
import type { CreatedJob } from "../src/generated/CreatedJob";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { Location } from "../src/generated/Location";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { AudioFormat } from "../src/generated/AudioFormat";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Card, Check, Cover, Empty, Pager } from "./components/ui";

export function ReleaseCard({ release }: { release: CatalogReleaseSummary }) {
  return (
    <a className="media-card album-card" href={`#/album/${encodeURIComponent(release.id)}`}>
      <Cover title={release.title} />
      <strong>{release.title}</strong>
      <span>{release.artist}</span>
      <small>
        {[release.date, release.country, release.disambiguation].filter(Boolean).join(" · ") ||
          "Edition details available"}
      </small>
    </a>
  );
}

export function Catalog({ term, artistId }: { term: string; artistId?: string | undefined }) {
  const [kind, setKind] = useState<"artists" | "releases">(artistId ? "releases" : "artists");
  const [offset, setOffset] = useState(0);
  const [expanded, setExpanded] = useState(false);
  const path = artistId
    ? `/api/discovery/artists/${encodeURIComponent(artistId)}/releases`
    : `/api/discovery/${kind}`;
  const result = useResource(
    (signal) =>
      request<CatalogResult<CatalogPage<CatalogArtist> | CatalogReleasePage>>(
        `${path}?${query({
          query: term,
          offset,
          limit: 24,
          ...(kind === "releases"
            ? {
                include_singles: expanded,
                include_live: expanded,
                include_compilations: expanded,
                include_unofficial: expanded,
              }
            : {}),
        })}`,
        { signal },
      ),
    `${path}:${term}:${offset}:${expanded}`,
  );
  const page = result.data?.data;
  const next =
    page && "next_offset" in page
      ? page.next_offset
      : page && offset + page.items.length < page.count
        ? offset + 24
        : null;
  return (
    <>
      <header className={`hero ${artistId ? "artist-hero" : ""}`}>
        <div>
          <h1>{artistId ? "Artist albums" : `Search${term ? ` · ${term}` : ""}`}</h1>
          <p>
            {artistId
              ? "Choose an exact release, not just an album title."
              : "Find artists and exact album editions on MusicBrainz."}
          </p>
          {artistId && <p className="identifier">MusicBrainz artist · {artistId}</p>}
        </div>
      </header>
      <div className="actions">
        {!artistId &&
          (["artists", "releases"] as const).map((value) => (
            <Button
              key={value}
              variant={kind === value ? "default" : "secondary"}
              aria-pressed={kind === value}
              onClick={() => {
                setKind(value);
                setOffset(0);
              }}
            >
              {value === "artists" ? "Artists" : "Albums / editions"}
            </Button>
          ))}
        {kind === "releases" && (
          <Check
            checked={expanded}
            onChange={(value) => {
              setExpanded(value);
              setOffset(0);
            }}
          >
            Include singles, live, compilations and unofficial editions
          </Check>
        )}
      </div>
      <Alert error={result.error} />
      {result.loading && <p role="status">Searching MusicBrainz…</p>}
      {result.data && (
        <p className="muted">
          {result.data.source} · {result.data.cached ? "Cached provider result" : "Provider result"}{" "}
          · {page?.count} provider rows
        </p>
      )}
      <div className={`media-grid ${kind === "artists" ? "artist-grid" : ""}`}>
        {page?.items.map((item) =>
          "name" in item ? (
            <a
              className="media-card artist-card"
              key={item.id}
              href={`#/artist/${encodeURIComponent(item.id)}`}
            >
              <Cover title={item.name} />
              <strong>{item.name}</strong>
              <span>
                {[item.country, item.kind, item.disambiguation].filter(Boolean).join(" · ") ||
                  "Artist"}
              </span>
            </a>
          ) : (
            <ReleaseCard key={item.id} release={item} />
          ),
        )}
      </div>
      {page?.items.length === 0 && (
        <Empty>No results on this page. A filtered page can still have a next page.</Empty>
      )}
      <Pager
        busy={result.loading}
        previous={offset > 0 ? () => setOffset(Math.max(0, offset - 24)) : undefined}
        next={next !== null && next !== undefined ? () => setOffset(next) : undefined}
      />
    </>
  );
}

export function Album({
  id,
  settings,
  locations,
  onCreated,
}: {
  id: string;
  settings: SettingsVersion | null | undefined;
  locations: Location[];
  onCreated: (job: number, version: number) => void;
}) {
  const release = useResource(
    (signal) =>
      request<CatalogResult<CatalogRelease>>(`/api/discovery/releases/${encodeURIComponent(id)}`, {
        signal,
      }),
    id,
  );
  const [selected, setSelected] = useState<number[]>([]);
  const [format, setFormat] = useState<AudioFormat>(settings?.settings.format ?? "mp3");
  const [coverage, setCoverage] = useState(false);
  const [created, setCreated] = useState<CreatedJob>();
  const command = useCommand();
  const album = release.data?.data;
  async function prepare() {
    if (!settings || !album) return;
    const job = created ?? (await request<CreatedJob>("/api/jobs", { body: {} }));
    setCreated(job);
    const body: CatalogPlanRequest = {
      job_id: job.job_id,
      expected_version: job.plan_version,
      settings_version: settings.version,
      format,
      locations: selected,
      accepts_incomplete_coverage: coverage,
      bucket: null,
    };
    const plan = await request<StoredPlan>(
      `/api/discovery/releases/${encodeURIComponent(id)}/plans`,
      { body },
    );
    onCreated(plan.job_id, plan.version);
  }
  return (
    <>
      <Alert error={release.error} />
      {release.loading && <p role="status">Loading the exact release…</p>}
      {album && (
        <>
          <header className="hero album-hero">
            <Cover title={album.summary.title} />
            <div>
              <h1>{album.summary.title}</h1>
              <p>
                {album.summary.artist} ·{" "}
                {[album.summary.date, album.summary.country, album.summary.status]
                  .filter(Boolean)
                  .join(" · ")}
              </p>
              <p>{album.summary.disambiguation}</p>
              <p className="identifier">Exact release · {album.summary.id}</p>
            </div>
          </header>
          <div className="detail-grid">
            <section>
              <h2>Tracks on this edition</h2>
              {album.media.map((medium) => (
                <div key={medium.position}>
                  <h3>
                    Disc {medium.position} · {medium.format ?? "Format not supplied"}
                  </h3>
                  {(medium.has_pregap || medium.has_data_tracks) && (
                    <p className="warning">
                      This medium has pregap or data tracks. The server will check whether it can
                      prepare a complete plan.
                    </p>
                  )}
                  <ol className="track-list">
                    {medium.tracks.map((track) => (
                      <li key={track.track_id}>
                        <span className="number">{track.number}</span>
                        <span>
                          {track.title}
                          <small>{track.credits.map((credit) => credit.name).join(", ")}</small>
                        </span>
                        <span className="muted">
                          {track.duration_ms === null
                            ? "—"
                            : `${Math.floor(track.duration_ms / 60000)}:${String(Math.floor(track.duration_ms / 1000) % 60).padStart(2, "0")}`}
                        </span>
                      </li>
                    ))}
                  </ol>
                </div>
              ))}
            </section>
            <Card title="Prepare your request">
              {!settings && (
                <p className="warning">
                  Output settings are missing. Run the local setup command before you create a
                  request.
                </p>
              )}
              {locations.length === 0 && (
                <p className="warning">No library roots are configured. Run local setup first.</p>
              )}
              <label>
                Output format
                <select
                  value={format}
                  disabled={command.busy}
                  onChange={(event) => setFormat(event.target.value as AudioFormat)}
                >
                  <option value="mp3">MP3 · 320 kbps</option>
                  <option value="flac">FLAC</option>
                </select>
              </label>
              <fieldset disabled={command.busy}>
                <legend>Configured destinations</legend>
                {locations.map((location) => (
                  <Check
                    key={location.id}
                    checked={selected.includes(location.id)}
                    onChange={(value) =>
                      setSelected((current) =>
                        value
                          ? [...current, location.id]
                          : current.filter((id) => id !== location.id),
                      )
                    }
                  >
                    <strong>{location.kind}</strong> · {location.root}
                    <small>
                      Coverage: {location.coverage}
                      {location.last_error ? ` · ${location.last_error}` : ""}
                    </small>
                  </Check>
                ))}
              </fieldset>
              <Check checked={coverage} onChange={setCoverage} disabled={command.busy}>
                I accept incomplete index coverage for these destinations. Existing files are never
                overwritten.
              </Check>
              <p className="muted">
                Settings version {settings?.version ?? "not configured"}. Preparing a plan does not
                approve it or start a download.
              </p>
              <Alert error={command.error} />
              {created && (
                <p>
                  Request #{created.job_id} exists. If preparation failed, try again here or inspect
                  it in the queue.
                </p>
              )}
              <Button
                disabled={command.busy || !settings || selected.length === 0 || !album}
                onClick={() => void command.run(prepare)}
              >
                {command.busy ? "Preparing…" : "Prepare output plan"}
              </Button>
            </Card>
          </div>
        </>
      )}
    </>
  );
}

export function Settings({
  settings,
  locations,
}: {
  settings: SettingsVersion | null | undefined;
  locations: Location[];
}) {
  return (
    <>
      <header className="hero">
        <div>
          <h1>Your setup</h1>
          <p>
            Read-only configuration. Local setup owns root registration and versioned output
            settings.
          </p>
        </div>
      </header>
      <Card title="Output settings">
        {settings ? (
          <dl className="properties">
            <dt>Saved version</dt>
            <dd>{settings.version}</dd>
            <dt>Default format</dt>
            <dd>{settings.settings.format}</dd>
            <dt>Folder template</dt>
            <dd>
              <code>{settings.settings.template.folder}</code>
            </dd>
            <dt>Filename template</dt>
            <dd>
              <code>{settings.settings.template.filename}</code>
            </dd>
          </dl>
        ) : (
          <Empty>Output settings are missing. Run local setup.</Empty>
        )}
      </Card>
      <Card title="Library roots">
        {locations.length ? (
          locations.map((location) => (
            <div className="list-row" key={location.id}>
              <div>
                <strong>{location.root}</strong>
                <p>
                  {location.kind} · Coverage: {location.coverage}
                </p>
                {location.last_error && <p className="warning">{location.last_error}</p>}
              </div>
              <Badge>{location.native_identity ? "Bound root" : "Binding required"}</Badge>
            </div>
          ))
        ) : (
          <Empty>No roots are configured. Run local setup.</Empty>
        )}
      </Card>
    </>
  );
}
