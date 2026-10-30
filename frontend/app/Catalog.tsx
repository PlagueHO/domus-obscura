import { useEffect, useRef, useState } from "react";
import type { CatalogArtist } from "../src/generated/CatalogArtist";
import type { CatalogRelease } from "../src/generated/CatalogRelease";
import type { CatalogReleasePage } from "../src/generated/CatalogReleasePage";
import type { CatalogReleaseSummary } from "../src/generated/CatalogReleaseSummary";
import type { CatalogResult } from "../src/generated/CatalogResult";
import type { CatalogPlanRequest } from "../src/generated/CatalogPlanRequest";
import type { CreatedJob } from "../src/generated/CreatedJob";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { SystemSettingsPolicy } from "../src/generated/SystemSettingsPolicy";
import type { OutputSettings } from "../src/generated/OutputSettings";
import type { Location } from "../src/generated/Location";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { AudioProfile } from "../src/generated/AudioProfile";
import type { RegisterLocation } from "../src/generated/RegisterLocation";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Check, Cover, Empty, Pager } from "./components/ui";
import { AlbumCard } from "./components/Media";
import { searchGap } from "./Search";
import { BackendConfiguration, ConfigurationCard } from "./BackendConfiguration";
import { LocalMatches } from "./LocalMatches";
import { AudioProfileSelect, audioProfileLabel, outputWithAudioPolicy } from "./audio-profiles";

export function ReleaseCard({ release }: { release: CatalogReleaseSummary }) {
  return <AlbumCard release={release} />;
}

export function Artist({ id, locations = [] }: { id: string; locations?: Location[] }) {
  const [albumsReady, setAlbumsReady] = useState(false);
  const artist = useResource(
    (signal) =>
      request<CatalogResult<CatalogArtist>>(`/api/discovery/artists/${encodeURIComponent(id)}`, {
        signal,
      }),
    id,
  );
  const identity = artist.data?.data;
  useEffect(() => {
    if (!artist.loading) setAlbumsReady(true);
  }, [artist.loading]);
  return (
    <>
      <header className="hero artist-hero album-hero">
        <Cover title={identity?.name ?? id} />
        <div>
          <h1>{identity?.name ?? "Artist"}</h1>
          <p>
            {[identity?.country, identity?.kind, identity?.disambiguation]
              .filter(Boolean)
              .join(" · ")}
          </p>
          <p className="identifier">MusicBrainz artist · {id}</p>
          <div className="actions">
            <Button variant="secondary" disabled={artist.loading} onClick={artist.refresh}>
              Reload artist details
            </Button>
            <a
              className="button button-secondary"
              href={`https://musicbrainz.org/artist/${encodeURIComponent(id)}`}
              target="_blank"
              rel="noreferrer"
            >
              MusicBrainz
            </a>
          </div>
        </div>
      </header>
      <Alert error={artist.error} />
      {artist.loading && <p role="status">Loading artist identity…</p>}
      {identity && (
        <LocalMatches locations={locations} queries={[{ artist: identity.name, album: null }]} />
      )}
      {albumsReady && <ArtistAlbums key={id} id={id} />}
    </>
  );
}

function ArtistAlbums({ id }: { id: string }) {
  const [pages, setPages] = useState([0]);
  const offset = pages.at(-1) ?? 0;
  const [expanded, setExpanded] = useState(false);
  const result = useResource(async (signal) => {
    await searchGap(signal);
    return request<CatalogResult<CatalogReleasePage>>(
      `/api/discovery/artists/${encodeURIComponent(id)}/releases?${query({
        offset,
        limit: 24,
        include_singles: expanded,
        include_live: expanded,
        include_compilations: expanded,
        include_unofficial: expanded,
      })}`,
      { signal },
    );
  }, `${id}:${offset}:${expanded}`);
  const page = result.data?.data;
  const next = page?.next_offset;
  return (
    <section className="search-group" aria-labelledby="artist-albums-heading">
      <header className="search-group-header">
        <h2 id="artist-albums-heading">Albums and exact editions</h2>
        <Button variant="ghost" disabled={result.loading} onClick={result.refresh}>
          Reload albums
        </Button>
      </header>
      <p>Choose an exact release, not just an album title. Each card opens its full track list.</p>
      <Check
        checked={expanded}
        onChange={(value) => {
          setExpanded(value);
          setPages([0]);
        }}
      >
        Include singles, live, compilations and unofficial editions
      </Check>
      <Alert error={result.error} />
      {result.loading && <p role="status">Searching MusicBrainz…</p>}
      {result.data && (
        <p className="muted">
          {result.data.source} · {result.data.cached ? "Cached provider result" : "Provider result"}{" "}
          · {page?.count} provider rows
        </p>
      )}
      <div className="media-grid">
        {page?.items.map((item) => (
          <ReleaseCard key={item.id} release={item} />
        ))}
      </div>
      {page?.items.length === 0 && (
        <Empty>No results on this page. A filtered page can still have a next page.</Empty>
      )}
      <Pager
        busy={result.loading}
        previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
        next={next !== null && next !== undefined ? () => setPages([...pages, next]) : undefined}
      />
    </section>
  );
}

export function editionLabel(release: CatalogReleaseSummary): string {
  return [
    release.title,
    release.date,
    release.country,
    release.disambiguation,
    release.status,
    release.id,
  ]
    .filter(Boolean)
    .join(" · ");
}

export function EditionChoices({
  current,
  items,
  disabled,
}: {
  current: CatalogReleaseSummary;
  items: CatalogReleaseSummary[];
  disabled: boolean;
}) {
  const choices = items.some((item) => item.id === current.id) ? items : [current, ...items];
  return (
    <label>
      Exact edition
      <select
        value={current.id}
        disabled={disabled}
        onChange={(event) => {
          if (choices.some((item) => item.id === event.target.value))
            window.location.hash = `/album/${encodeURIComponent(event.target.value)}`;
        }}
      >
        {choices.map((item) => (
          <option key={item.id} value={item.id}>
            {editionLabel(item)}
          </option>
        ))}
      </select>
    </label>
  );
}

function EditionBrowser({ current, group }: { current: CatalogReleaseSummary; group: string }) {
  const [pages, setPages] = useState([0]);
  const offset = pages.at(-1) ?? 0;
  const [unofficial, setUnofficial] = useState(false);
  const editions = useResource(async (signal) => {
    await searchGap(signal);
    return request<CatalogResult<CatalogReleasePage>>(
      `/api/discovery/release-groups/${encodeURIComponent(group)}/releases?${query({
        offset,
        limit: 24,
        include_singles: true,
        include_live: true,
        include_compilations: true,
        include_unofficial: unofficial,
      })}`,
      { signal },
    );
  }, `${group}:${offset}:${unofficial}`);
  const next = editions.data?.data.next_offset;
  return (
    <div className="edition-browser">
      <Alert error={editions.error} />
      {editions.loading && <p role="status">Loading editions from this exact release group…</p>}
      {!editions.loading && !editions.error && editions.data?.data.items.length === 0 && (
        <p>No editions on this provider page match the selected filters.</p>
      )}
      <EditionChoices
        current={current}
        items={editions.data?.data.items ?? []}
        disabled={editions.loading || Boolean(editions.error)}
      />
      <Check
        checked={unofficial}
        onChange={(value) => {
          setUnofficial(value);
          setPages([0]);
        }}
      >
        Include unofficial editions
      </Check>
      <p className="muted">
        These editions belong to the same provider release group. This does not prove the peer's
        edition or permission to download. No edition is automatically recommended or approved.
      </p>
      <div className="actions">
        <Button variant="secondary" disabled={editions.loading} onClick={editions.refresh}>
          Reload editions
        </Button>
        <Pager
          busy={editions.loading}
          previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
          next={next !== null && next !== undefined ? () => setPages([...pages, next]) : undefined}
        />
      </div>
    </div>
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
  const drawer = useRef<HTMLDialogElement>(null);
  const release = useResource(
    (signal) =>
      request<CatalogResult<CatalogRelease>>(`/api/discovery/releases/${encodeURIComponent(id)}`, {
        signal,
      }),
    id,
  );
  const [selected, setSelected] = useState<number[]>([]);
  const [selectedProfile, setProfile] = useState<AudioProfile>();
  const profile = selectedProfile ?? settings?.policy.preferred;
  const accepted = settings?.policy.accepted ?? [];
  const [coverage, setCoverage] = useState(false);
  const [created, setCreated] = useState<CreatedJob>();
  const [editionsOpen, setEditionsOpen] = useState(false);
  const command = useCommand();
  const album = release.data?.data;
  async function prepare() {
    if (!settings || !album || !profile || !accepted.includes(profile)) return;
    const job = created ?? (await request<CreatedJob>("/api/jobs", { body: {} }));
    setCreated(job);
    const body: CatalogPlanRequest = {
      job_id: job.job_id,
      expected_version: job.plan_version,
      settings_version: settings.version,
      format:
        settings.audio_profiles.find((descriptor) => descriptor.profile === profile)?.format ??
        null,
      audio_profile: profile,
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
              <div className="actions">
                <Button onClick={() => drawer.current?.showModal()}>Start Download</Button>
                {album.release_group_id && (
                  <Button
                    variant="secondary"
                    aria-expanded={editionsOpen}
                    onClick={() => setEditionsOpen(!editionsOpen)}
                  >
                    {editionsOpen ? "Hide editions" : "Choose another edition"}
                  </Button>
                )}
                <a
                  className="button button-secondary"
                  href={`https://musicbrainz.org/release/${encodeURIComponent(album.summary.id)}`}
                  target="_blank"
                  rel="noreferrer"
                >
                  MusicBrainz
                </a>
              </div>
            </div>
          </header>
          {editionsOpen && album.release_group_id && (
            <EditionBrowser current={album.summary} group={album.release_group_id} />
          )}
          <LocalMatches
            locations={locations}
            queries={[{ artist: album.summary.artist, album: album.summary.title }]}
          />
          <div>
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
            <dialog ref={drawer} className="download-drawer" aria-labelledby="download-title">
              <header className="design-card-header">
                <h2 id="download-title">Download album</h2>
                <Button variant="ghost" onClick={() => drawer.current?.close()}>
                  Close
                </Button>
              </header>
              <div className="design-card-body">
                <h3>{album.summary.title}</h3>
                <p>
                  {album.summary.artist} ·{" "}
                  {[album.summary.date, album.summary.country].filter(Boolean).join(" · ")}
                </p>
                <p className="identifier">Exact edition · {album.summary.id}</p>
                <p>
                  First save this exact edition and its destinations. Then review the source and
                  exact cover before the separate download approval.
                </p>
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
                  Output audio profile
                  <AudioProfileSelect
                    value={profile}
                    accepted={accepted}
                    descriptors={settings?.audio_profiles ?? []}
                    disabled={command.busy}
                    onChange={setProfile}
                  />
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
                  I accept incomplete index coverage for these destinations. Existing files are
                  never overwritten.
                </Check>
                <p className="muted">
                  Settings version {settings?.version ?? "not configured"}. Preparing a plan does
                  not approve it or start a download.
                </p>
                <Alert error={command.error} />
                {created && (
                  <p>
                    Request #{created.job_id} exists. If preparation failed, try again here or
                    inspect it in the queue.
                  </p>
                )}
                <Button
                  disabled={
                    command.busy ||
                    !settings ||
                    !profile ||
                    !accepted.includes(profile) ||
                    selected.length === 0 ||
                    !album
                  }
                  onClick={() => void command.run(prepare)}
                >
                  {command.busy ? "Preparing…" : "Prepare output plan"}
                </Button>
              </div>
            </dialog>
          </div>
        </>
      )}
    </>
  );
}

export function Settings({
  settings,
  locations,
  policy,
  onSaved,
  error,
  loading = false,
  onReload,
}: {
  settings: SettingsVersion | null | undefined;
  locations: Location[];
  policy: SystemSettingsPolicy | undefined;
  onSaved: () => void;
  error?: unknown;
  loading?: boolean;
  onReload: () => void;
}) {
  const [savedVersion, setSavedVersion] = useState<number>();
  const [outputDirty, setOutputDirty] = useState(false);
  const [rootDirty, setRootDirty] = useState(false);
  const readOnly = policy?.read_only ?? true;

  return (
    <>
      <header className="page-heading">
        <div>
          <h1>System settings</h1>
          <p className="muted">Manage the application and shared Music library.</p>
        </div>
      </header>
      <BackendConfiguration
        onRefreshRelated={onReload}
        relatedLoading={loading}
        otherDrafts={outputDirty || rootDirty}
        output={
          <ConfigurationCard title="Output policy">
            <Alert error={error} />
            {savedVersion !== undefined && (
              <p role="status">Output policy saved. New plans use version {savedVersion}.</p>
            )}
            {settings ? (
              <>
                <p>
                  These settings apply to the shared library. New plans use saved changes
                  immediately. Existing plans keep their frozen settings.
                </p>
                {readOnly ? (
                  <>
                    <p role="status">
                      This form is read-only. The administrator controls this mode in the server
                      JSON configuration.
                    </p>
                    <dl className="properties">
                      <dt>Accepted audio profiles</dt>
                      <dd>
                        {settings.policy.accepted
                          .map((profile) => audioProfileLabel(profile, settings.audio_profiles))
                          .join("; ")}
                      </dd>
                      <dt>Preferred audio profile</dt>
                      <dd>
                        {audioProfileLabel(settings.policy.preferred, settings.audio_profiles)}
                      </dd>
                      <dt>Artwork size</dt>
                      <dd>
                        {settings.settings.artwork_size === "small"
                          ? "200 × 200 px"
                          : settings.settings.artwork_size === "recommended"
                            ? "400 × 400 px"
                            : settings.settings.artwork_size === "large"
                              ? "800 × 800 px"
                              : "Legacy · up to 1200 × 1200 px"}
                      </dd>
                      <dt>Album folder template</dt>
                      <dd>
                        <code>{settings.settings.template.folder}</code>
                      </dd>
                      <dt>Track filename template</dt>
                      <dd>
                        <code>{settings.settings.template.filename}</code>
                      </dd>
                    </dl>
                  </>
                ) : (
                  <SystemOutputForm
                    key={settings.version}
                    settings={settings}
                    onDirty={setOutputDirty}
                    disabled={loading || Boolean(error)}
                    onSaved={(version) => {
                      setSavedVersion(version);
                      onSaved();
                    }}
                  />
                )}
              </>
            ) : loading ? (
              <p role="status">Loading system output settings…</p>
            ) : error ? (
              <Empty>
                System output settings could not be loaded. Reload system settings to try again.
              </Empty>
            ) : (
              <Empty>Output settings are missing. Run local setup.</Empty>
            )}
          </ConfigurationCard>
        }
        roots={
          <ConfigurationCard title="Library roots">
            <Alert error={error} />
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
            ) : loading ? (
              <p role="status">Loading library roots…</p>
            ) : error ? (
              <Empty>Library roots could not be loaded. Reload system settings to try again.</Empty>
            ) : (
              <Empty>No library roots are configured.</Empty>
            )}
            {!readOnly && (
              <LibraryRootControls
                key={locations.map((location) => location.id).join(":")}
                locations={locations}
                onDirty={setRootDirty}
                disabled={loading || Boolean(error)}
                onChanged={onReload}
              />
            )}
            <p className="muted">
              Roots are folders on the server, not on this browser device. Register existing folders
              before using them. Registration binds their filesystem identity and does not move
              media or start a scan. Existing roots cannot be edited, removed or rebound here
              because saved plans and receipts depend on them.
            </p>
          </ConfigurationCard>
        }
      />
    </>
  );
}

function SystemOutputForm({
  settings,
  disabled,
  onSaved,
  onDirty,
}: {
  settings: SettingsVersion;
  disabled: boolean;
  onSaved: (version: number) => void;
  onDirty: (dirty: boolean) => void;
}) {
  const [draft, setDraft] = useState<OutputSettings>(settings.settings);
  const [audioPolicy, setAudioPolicy] = useState(settings.policy);
  const [submitted, setSubmitted] = useState(false);
  const command = useCommand();
  const saving = disabled || command.busy || submitted;
  const accepted = audioPolicy.accepted;
  const validAudio =
    accepted.length > 0 &&
    accepted.includes(audioPolicy.preferred) &&
    accepted.every((profile) =>
      settings.audio_profiles.some((descriptor) => descriptor.profile === profile),
    );
  useEffect(() => {
    onDirty(
      !submitted &&
        (JSON.stringify(draft) !== JSON.stringify(settings.settings) ||
          JSON.stringify(audioPolicy) !== JSON.stringify(settings.policy)),
    );
    return () => onDirty(false);
  }, [draft, audioPolicy, settings.settings, settings.policy, submitted, onDirty]);

  async function save() {
    if (saving || !validAudio) return;
    const version = await request<number>("/api/settings", {
      method: "PUT",
      body: {
        expected_version: settings.version,
        settings: outputWithAudioPolicy(draft, audioPolicy, settings.audio_profiles),
      },
    });
    setSubmitted(true);
    onSaved(version);
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void command.run(save);
      }}
    >
      <fieldset disabled={saving}>
        <legend>Accepted audio profiles</legend>
        <p>Select every profile the shared library accepts.</p>
        {settings.audio_profiles.map((choice) => (
          <Check
            key={choice.profile}
            checked={accepted.includes(choice.profile)}
            onChange={(checked) =>
              setAudioPolicy((current) => ({
                ...current,
                accepted: checked
                  ? [...current.accepted, choice.profile]
                  : current.accepted.filter((profile) => profile !== choice.profile),
              }))
            }
          >
            {choice.label}
          </Check>
        ))}
        <p className="muted">
          MP3 VBR requires actual bitrate variation. It has no preset or average-bitrate target.
          FLAC is lossless and has no fixed bitrate. M4A is not supported.
        </p>
      </fieldset>
      <label>
        Preferred audio profile
        <AudioProfileSelect
          value={audioPolicy.preferred}
          accepted={accepted}
          descriptors={settings.audio_profiles}
          disabled={saving}
          onChange={(preferred) => setAudioPolicy((current) => ({ ...current, preferred }))}
        />
        <small>
          New album requests start with this choice. Other accepted choices remain available. No
          download or fallback starts automatically.
        </small>
      </label>
      {!validAudio && (
        <p role="alert">
          Select at least one accepted profile, then select a preferred profile from that selection.
        </p>
      )}
      <label>
        Artwork size
        <select
          value={draft.artwork_size ?? "legacy"}
          disabled={saving}
          onChange={(event) => {
            const value = event.target.value;
            if (value === "small" || value === "recommended" || value === "large")
              setDraft((current) => ({ ...current, artwork_size: value }));
          }}
        >
          <option value="small">200 × 200 px</option>
          <option value="recommended">400 × 400 px · Recommended</option>
          <option value="large">800 × 800 px</option>
          {draft.artwork_size === null && (
            <option value="legacy">Legacy · up to 1200 × 1200 px</option>
          )}
        </select>
        <small>This is a maximum size. The image keeps its shape and is never enlarged.</small>
      </label>
      <label>
        Album folder template
        <input
          value={draft.template.folder}
          disabled={saving}
          onChange={(event) =>
            setDraft((current) => ({
              ...current,
              template: { ...current.template, folder: event.target.value },
            }))
          }
        />
      </label>
      <label>
        Track filename template
        <input
          value={draft.template.filename}
          disabled={saving}
          onChange={(event) =>
            setDraft((current) => ({
              ...current,
              template: { ...current.template, filename: event.target.value },
            }))
          }
        />
      </label>
      <p>
        Template changes are validated by the server. They do not change saved plans or rename
        existing files.
      </p>
      <Alert error={command.error} />
      {Boolean(command.error) && (
        <p>
          Your edits are kept. If the saved version changed, reload system settings before trying
          again.
        </p>
      )}
      <Button type="submit" disabled={saving || !validAudio}>
        {command.busy ? "Saving…" : "Save system output settings"}
      </Button>
    </form>
  );
}

function LibraryRootControls({
  locations,
  disabled,
  onChanged,
  onDirty,
}: {
  locations: Location[];
  disabled: boolean;
  onChanged: () => void;
  onDirty: (dirty: boolean) => void;
}) {
  const hasMain = locations.some((location) => location.kind === "main");
  const [root, setRoot] = useState("");
  const [kind, setKind] = useState<"main" | "backup">(hasMain ? "backup" : "main");
  const [submitted, setSubmitted] = useState(false);
  const [message, setMessage] = useState("");
  const command = useCommand();
  const busy = disabled || command.busy;
  useEffect(() => {
    onDirty(!submitted && root.length > 0);
    return () => onDirty(false);
  }, [root, submitted, onDirty]);

  async function register() {
    if (busy || submitted) return;
    const body: RegisterLocation = { root, kind };
    await request<Location>("/api/locations", { body });
    setSubmitted(true);
    onChanged();
  }

  async function scan(id: number) {
    if (busy) return;
    setMessage("");
    await request<Location>(`/api/locations/${id}/scan`, { body: {} });
    setMessage(
      "The server accepted the scan request. Refresh the saved index to check progress. No media files were changed.",
    );
    onChanged();
  }

  return (
    <>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void command.run(register);
        }}
      >
        <h3>Add a library root</h3>
        <label>
          Server folder path
          <input
            value={root}
            onChange={(event) => setRoot(event.target.value)}
            required
            maxLength={4096}
            disabled={busy || submitted}
            placeholder="Absolute folder path on the server"
          />
        </label>
        <label>
          Library role
          <select
            value={kind}
            disabled={busy || submitted}
            onChange={(event) => {
              const value = event.target.value;
              if (value === "main" || value === "backup") setKind(value);
            }}
          >
            {!hasMain && <option value="main">Main library</option>}
            <option value="backup">Backup library</option>
          </select>
        </label>
        <Button
          type="submit"
          disabled={busy || submitted || !root.trim() || locations.length >= 100}
        >
          Register library root
        </Button>
        {submitted && (
          <p role="status">Library root registered. Refresh Music policy and roots to view it.</p>
        )}
      </form>
      {locations.length > 0 && (
        <fieldset disabled={busy}>
          <legend>Read-only library indexing</legend>
          <p>
            Scan registered folders to update index coverage. A scan does not change media files.
          </p>
          {locations.map((location) => (
            <Button
              key={location.id}
              variant="secondary"
              onClick={() => void command.run(() => scan(location.id))}
            >
              Scan {location.kind} root #{location.id}
            </Button>
          ))}
        </fieldset>
      )}
      {command.busy && <p role="status">Updating library configuration or index…</p>}
      {message && <p role="status">{message}</p>}
      <Alert error={command.error} />
    </>
  );
}
