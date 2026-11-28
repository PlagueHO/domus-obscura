import { useEffect, useId, useRef, useState } from "react";
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
import type { LibrarySetup } from "../src/generated/LibrarySetup";
import { query, request } from "./api";
import { useCommand, useMetadataResource } from "./hooks";
import {
  Alert,
  Button,
  Check,
  Cover,
  Empty,
  EmptyState,
  HeaderCard,
  HeaderIconAction,
  LoadingState,
  Pager,
  SettingsRow,
  Subcard,
  Switch,
  SettingsSaveButton,
} from "./components/ui";
import { AlbumCard, Icon } from "./components/Media";
import { ArtistPhoto } from "./ArtistPhoto";
import { BackendConfiguration, ConfigurationCard } from "./BackendConfiguration";
import { LocalMatches } from "./LocalMatches";
import { AudioProfileSelect, audioProfileLabel, outputWithAudioPolicy } from "./audio-profiles";
import { SelectAlbum } from "./AlbumSelection";
import { defaultTagPolicy, FrozenTagPolicy, sameTagPolicy, TagPolicyFields } from "./TagPolicy";
import { LibraryFolderRegistration, LibraryFolderTable } from "./LibrarySetup";
import { LibraryFolderChange, useLibraryFolderControl } from "./PrimaryLibrary";

export function ReleaseCard({ release }: { release: CatalogReleaseSummary }) {
  return <AlbumCard release={release} />;
}

export function Artist({ id, locations = [] }: { id: string; locations?: Location[] }) {
  const [albumsReady, setAlbumsReady] = useState(false);
  const artist = useMetadataResource(
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
        <ArtistPhoto key={id} id={id} title={identity?.name ?? id} />
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
            <a
              className="button button-secondary"
              href={`#/settings?section=music_providers&artist=${encodeURIComponent(id)}`}
            >
              Manage artist photo
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
  const result = useMetadataResource(async (signal) => {
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
  const editions = useMetadataResource(async (signal) => {
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
  const release = useMetadataResource(
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
  const primaryScanOnly = locations.some(
    (location) => location.kind === "main" && location.scan_only,
  );
  async function prepare() {
    if (!settings || !album || !profile || !accepted.includes(profile) || primaryScanOnly) return;
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
                <SelectAlbum release={album.summary} />
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
                {primaryScanOnly && (
                  <p className="warning">
                    Acquisition requires a primary library that allows writes.
                  </p>
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
                      disabled={Boolean(location.scan_only)}
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
                        {location.scan_only ? "Scan only" : `Coverage: ${location.coverage}`}
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
                    primaryScanOnly ||
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
  librarySetup,
  onDismiss,
}: {
  settings: SettingsVersion | null | undefined;
  locations: Location[];
  policy: SystemSettingsPolicy | undefined;
  onSaved: () => void;
  error?: unknown;
  loading?: boolean;
  onReload: () => void;
  librarySetup?: LibrarySetup | undefined;
  onDismiss?: () => void;
}) {
  const [savedVersion, setSavedVersion] = useState<number>();
  const readOnly = policy?.read_only ?? true;

  return (
    <>
      <header className="settings-title">
        <h1 id="system-settings-title">System Settings</h1>
        {onDismiss && (
          <HeaderIconAction label="Close settings" onClick={onDismiss}>
            <Icon name="close" />
          </HeaderIconAction>
        )}
      </header>
      <div className="settings-intro">
        <p className="muted">Manage the application and shared Music library.</p>
        <a href="#/setup">Open setup wizard</a>
      </div>
      <BackendConfiguration
        onRefreshRelated={onReload}
        relatedLoading={loading}
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
                    <FrozenTagPolicy policy={settings.settings.tag_policy ?? defaultTagPolicy()} />
                  </>
                ) : (
                  <SystemOutputForm
                    key={settings.version}
                    settings={settings}
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
              <Empty>
                Output settings are missing. Open the setup wizard to save the initial policy.
              </Empty>
            )}
          </ConfigurationCard>
        }
        roots={
          <LibraryFoldersCard
            error={error}
            loading={loading}
            locations={locations}
            librarySetup={librarySetup}
            readOnly={readOnly}
            onReload={onReload}
          />
        }
      />
    </>
  );
}

function LibraryFoldersCard({
  error,
  loading,
  locations,
  librarySetup,
  readOnly,
  onReload,
}: {
  error: unknown;
  loading: boolean;
  locations: Location[];
  librarySetup: LibrarySetup | undefined;
  readOnly: boolean;
  onReload: () => void;
}) {
  const hasPrimary = locations.some((location) => location.kind === "main");
  const [adding, setAdding] = useState(false);
  const [changing, setChanging] = useState(false);
  const control = useLibraryFolderControl();
  const primaries = locations.filter((location) => location.kind === "main");
  const backups = locations.filter((location) => location.kind !== "main");
  return (
    <HeaderCard
      title="Library folders"
      className="configuration-section"
      actions={
        <HeaderIconAction href="#/library-scans" label="Open Library scans">
          <Icon name="scan" />
        </HeaderIconAction>
      }
    >
      <Alert error={error} />
      {loading && !locations.length ? (
        <LoadingState label="Loading library folders…" />
      ) : error && !locations.length ? (
        <EmptyState title="Library folders could not be loaded">
          Reload system settings to try again.
        </EmptyState>
      ) : (
        <div className="subcard-stack">
          <Subcard
            title="Primary library"
            actions={
              !readOnly &&
              hasPrimary && (
                <Button
                  variant="secondary"
                  width="medium"
                  icon={<Icon name={changing ? "close" : "folder"} />}
                  disabled={loading || Boolean(error)}
                  aria-expanded={changing}
                  onClick={() => {
                    setAdding(false);
                    setChanging((open) => !open);
                  }}
                >
                  {changing ? "Close" : "Change…"}
                </Button>
              )
            }
          >
            {hasPrimary ? (
              <LibraryFolderTable label="Current primary folder" locations={primaries} />
            ) : (
              <p className="subcard-note">No primary library selected.</p>
            )}
            {!readOnly && (
              <LibraryFolderChange
                hasPrimary={hasPrimary}
                control={control}
                open={changing}
                onOpenChange={setChanging}
                disabled={loading || Boolean(error)}
                onChanged={onReload}
              />
            )}
            {!readOnly && !hasPrimary && (
              <LibraryFolderRegistration
                key={locations.map((location) => location.id).join(":")}
                setup={librarySetup}
                locations={locations}
                disabled={loading || Boolean(error)}
                onChanged={onReload}
              />
            )}
          </Subcard>
          <Subcard
            title="Backup libraries"
            actions={
              !readOnly &&
              hasPrimary && (
                <Button
                  variant="secondary"
                  width="medium"
                  icon={<Icon name={adding ? "close" : "plus"} />}
                  disabled={loading || Boolean(error)}
                  aria-expanded={adding}
                  onClick={() => {
                    setChanging(false);
                    setAdding((open) => !open);
                  }}
                >
                  {adding ? "Close form" : "Add backup"}
                </Button>
              )
            }
          >
            {backups.length ? (
              <LibraryFolderTable label="Backup folders" locations={backups} />
            ) : (
              <p className="subcard-note">No backup libraries.</p>
            )}
            {!readOnly && hasPrimary && (
              <LibraryFolderChange
                backup
                hasPrimary
                control={control}
                open={adding}
                disabled={loading || Boolean(error)}
                onChanged={onReload}
              />
            )}
            {!readOnly &&
              hasPrimary &&
              adding &&
              control.data?.pending_change_id == null &&
              librarySetup?.folders.some((folder) => folder.location_id === null) && (
                <LibraryFolderRegistration
                  embedded
                  key={locations.map((location) => location.id).join(":")}
                  setup={librarySetup}
                  locations={locations}
                  disabled={loading || Boolean(error)}
                  onChanged={() => {
                    setAdding(false);
                    onReload();
                  }}
                />
              )}
          </Subcard>
        </div>
      )}
      <p className="muted">Changes never move media or start a scan.</p>
    </HeaderCard>
  );
}

function SystemOutputForm({
  settings,
  disabled,
  onSaved,
}: {
  settings: SettingsVersion;
  disabled: boolean;
  onSaved: (version: number) => void;
}) {
  const formId = useId();
  const [draft, setDraft] = useState<OutputSettings>(settings.settings);
  const [audioPolicy, setAudioPolicy] = useState(settings.policy);
  const [submitted, setSubmitted] = useState(false);
  const command = useCommand();
  const saving = disabled || command.busy || submitted;
  const accepted = audioPolicy.accepted;
  const saved = settings.settings;
  const dirty =
    draft.artwork_size !== saved.artwork_size ||
    draft.template.folder !== saved.template.folder ||
    draft.template.filename !== saved.template.filename ||
    audioPolicy.preferred !== settings.policy.preferred ||
    accepted.length !== settings.policy.accepted.length ||
    accepted.some((profile) => !settings.policy.accepted.includes(profile)) ||
    !sameTagPolicy(draft.tag_policy ?? defaultTagPolicy(), saved.tag_policy ?? defaultTagPolicy());
  const validAudio =
    accepted.length > 0 &&
    accepted.includes(audioPolicy.preferred) &&
    accepted.every((profile) =>
      settings.audio_profiles.some((descriptor) => descriptor.profile === profile),
    );
  async function save() {
    if (saving || !dirty || !validAudio) return;
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
      id={formId}
      onSubmit={(event) => {
        event.preventDefault();
        void command.run(save);
      }}
    >
      <fieldset className="audio-profile-options" disabled={saving}>
        <legend>Accepted audio profiles</legend>
        <p>Select every profile the shared library accepts.</p>
        {settings.audio_profiles.map((choice) => (
          <SettingsRow key={choice.profile} label={choice.label}>
            {(control) => (
              <Switch
                {...control}
                checked={accepted.includes(choice.profile)}
                onChange={(checked) => {
                  setAudioPolicy((current) => ({
                    ...current,
                    accepted: checked
                      ? [...current.accepted, choice.profile]
                      : current.accepted.filter((profile) => profile !== choice.profile),
                  }));
                }}
              />
            )}
          </SettingsRow>
        ))}
        <p className="muted">
          MP3 VBR requires actual bitrate variation. It has no preset or average-bitrate target.
          FLAC is lossless and has no fixed bitrate. M4A is not supported.
        </p>
      </fieldset>
      <SettingsRow
        label="Preferred audio profile"
        hint="New album requests start with this choice. Other accepted choices remain available. No download or fallback starts automatically."
      >
        {(control) => (
          <AudioProfileSelect
            {...control}
            value={audioPolicy.preferred}
            accepted={accepted}
            descriptors={settings.audio_profiles}
            disabled={saving}
            onChange={(preferred) => setAudioPolicy((current) => ({ ...current, preferred }))}
          />
        )}
      </SettingsRow>
      {!validAudio && (
        <p role="alert">
          Select at least one accepted profile, then select a preferred profile from that selection.
        </p>
      )}
      <SettingsRow
        label="Artwork size"
        hint="This is a maximum size. The image keeps its shape and is never enlarged."
      >
        {(control) => (
          <select
            {...control}
            value={draft.artwork_size ?? "legacy"}
            disabled={saving}
            onChange={(event) => {
              const value = event.target.value;
              if (value === "small" || value === "recommended" || value === "large")
                setDraft((current) => ({ ...current, artwork_size: value }));
              else if (value === "legacy" && saved.artwork_size === null)
                setDraft((current) => ({ ...current, artwork_size: null }));
            }}
          >
            <option value="small">200 × 200 px</option>
            <option value="recommended">400 × 400 px · Recommended</option>
            <option value="large">800 × 800 px</option>
            {saved.artwork_size === null && (
              <option value="legacy">Legacy · up to 1200 × 1200 px</option>
            )}
          </select>
        )}
      </SettingsRow>
      <SettingsRow label="Album folder template" hint="Sets the folder name for new albums." wide>
        {(control) => (
          <input
            {...control}
            value={draft.template.folder}
            disabled={saving}
            onChange={(event) =>
              setDraft((current) => ({
                ...current,
                template: { ...current.template, folder: event.target.value },
              }))
            }
          />
        )}
      </SettingsRow>
      <TagPolicyFields
        policy={draft.tag_policy ?? defaultTagPolicy()}
        disabled={saving}
        onChange={(tag_policy) => setDraft((current) => ({ ...current, tag_policy }))}
      />
      <SettingsRow
        label="Track filename template"
        hint="Sets the file names for new tracks. The server validates templates without changing saved plans or renaming files."
        wide
      >
        {(control) => (
          <input
            {...control}
            value={draft.template.filename}
            disabled={saving}
            onChange={(event) =>
              setDraft((current) => ({
                ...current,
                template: { ...current.template, filename: event.target.value },
              }))
            }
          />
        )}
      </SettingsRow>
      <Alert error={command.error} />
      {Boolean(command.error) && (
        <p>
          Your edits are kept. If the saved version changed, reload system settings before trying
          again.
        </p>
      )}
      <SettingsSaveButton
        form={formId}
        type="submit"
        disabled={saving || !dirty || !validAudio}
        busy={command.busy}
      >
        Save system output settings
      </SettingsSaveButton>
    </form>
  );
}
