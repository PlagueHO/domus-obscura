import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { SessionIdentity } from "../src/generated/SessionIdentity";
import type { Location } from "../src/generated/Location";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { SystemSettingsPolicy } from "../src/generated/SystemSettingsPolicy";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredQuestion } from "../src/generated/StoredQuestion";
import type { LibrarySetup } from "../src/generated/LibrarySetup";
import { request, type LocalStatus } from "./api";
import { WorkerRequests } from "./WorkerRequests";
import { useCommand, useResource } from "./hooks";
import { activeStates } from "./state";
import {
  Alert,
  Badge,
  Button,
  Card,
  Check,
  CompactTable,
  Cover,
  Empty,
  EmptyState,
  HeaderCard,
  HeaderIconAction,
  LoadingState,
  Pager,
  ProviderWaiting,
  SettingsRow,
  SettingsSaveButton,
  StatTile,
  StatusBadge,
  Subcard,
  Switch,
} from "./components/ui";
import { Album, Artist, Settings } from "./Catalog";
import { RequestPage } from "./Request";
import { Library } from "./Library";
import { SetupWizard, needsLibrarySetup } from "./LibrarySetup";
import { LibraryMaintenance } from "./LibraryMaintenance";
import { Inbox } from "./Inbox";
import { Search } from "./Search";
import { Recovery, recoveryJobFilter } from "./Recovery";
import { AlbumSelection, AlbumSelectionProvider, useAlbumSelection } from "./AlbumSelection";
import {
  FocusHeader,
  HomeQueue,
  Icon,
  JobStatus,
  QueueCard,
  QueuePage,
  groupJobs,
} from "./components/Media";

function isSettingsRoute(route: string) {
  return route === "/settings" || route.startsWith("/settings?");
}

function useRoute() {
  const [route, setRoute] = useState(() => {
    const current = window.location.hash.slice(1) || "/";
    return { current, page: isSettingsRoute(current) ? "/" : current };
  });
  useEffect(() => {
    function change() {
      const current = window.location.hash.slice(1) || "/";
      setRoute((previous) => ({
        current,
        page: isSettingsRoute(current) ? previous.page : current,
      }));
    }
    window.addEventListener("hashchange", change);
    return () => window.removeEventListener("hashchange", change);
  }, []);
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [route.page]);
  return route;
}

function SettingsDialog({ children, onDismiss }: { children: ReactNode; onDismiss: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useLayoutEffect(() => {
    const element = dialog.current!;
    element.showModal();
    return () => element.close();
  }, []);
  return (
    <dialog
      ref={dialog}
      className="settings-dialog"
      aria-labelledby="system-settings-title"
      onCancel={(event) => {
        if (event.target !== event.currentTarget) return;
        event.preventDefault();
        onDismiss();
      }}
    >
      {children}
    </dialog>
  );
}

function ComponentLibrary() {
  const [checked, setChecked] = useState(false);
  const [lossless, setLossless] = useState(true);
  return (
    <>
      <header className="page-heading">
        <h1>Component library</h1>
        <p>Shared production components. This page makes no service commands.</p>
      </header>
      <Card title="Actions and input states">
        <div className="actions">
          <Button>Primary action</Button>
          <Button variant="secondary">Secondary action</Button>
          <Button variant="destructive">Safe cancel</Button>
          <Button disabled>Unavailable action</Button>
        </div>
        {(["small", "medium", "large"] as const).map((width) => (
          <div className="actions" key={width}>
            <Button width={width}>
              {width === "small" ? "Small" : width === "medium" ? "Medium" : "Large"}
            </Button>
            <Button width={width} icon={<Icon name="refresh" />} variant="secondary">
              Refresh
            </Button>
          </div>
        ))}
        <Button width="medium" icon={<Icon name="refresh" />} variant="secondary" busy>
          Refresh
        </Button>
        <Check checked={checked} onChange={setChecked}>
          Keyboard-accessible explicit acknowledgement
        </Check>
        <div className="settings-save-footer">
          <SettingsSaveButton disabled>Save settings</SettingsSaveButton>
        </div>
      </Card>
      <Card title="Status and empty states">
        <div className="actions">
          <Badge>Pending review</Badge>
          {(["ready", "scanning", "pending", "attention", "failed"] as const).map((status) => (
            <StatusBadge key={status} status={status} />
          ))}
        </div>
        <Alert error={new Error("Example failure message")} />
        <EmptyState title="Nothing here yet" action={<Button variant="secondary">Add one</Button>}>
          Say what is missing and what to do next.
        </EmptyState>
        <LoadingState label="Loading example" />
      </Card>
      <Card title="Settings rows and stat tiles">
        <SettingsRow label="Primary library" hint="Where new albums are published.">
          <Button variant="secondary">Change</Button>
        </SettingsRow>
        <SettingsRow label="Artwork size" hint="The maximum size for saved artwork.">
          {(control) => (
            <select {...control} defaultValue="400">
              <option value="400">400 × 400 px</option>
              <option value="800">800 × 800 px</option>
            </select>
          )}
        </SettingsRow>
        <SettingsRow label="Accept lossless audio">
          {(control) => <Switch {...control} checked={lossless} onChange={setLossless} />}
        </SettingsRow>
        <SettingsRow label="Album folder template" hint="The pattern for new folder names." wide>
          {(control) => <input {...control} defaultValue="{album_artist}\{album}" />}
        </SettingsRow>
        <div className="actions">
          <StatTile label="Albums" value="1,284" />
          <StatTile label="Tracks" value="14,902" />
        </div>
      </Card>
      <Card title="Artwork unavailable">
        <div className="component-cover">
          <Cover title="Component example" />
        </div>
        <p>Missing artwork stays explicit. It never becomes a fabricated album cover.</p>
      </Card>
      <HeaderCard
        title="Header card"
        actions={
          <HeaderIconAction href="#/library-scans" label="Open Library scans">
            <Icon name="scan" />
          </HeaderIconAction>
        }
      >
        <p>A compact titled card with an optional body.</p>
      </HeaderCard>
      <HeaderCard title="Header-only card" actions={<Badge>No body</Badge>} />
      <HeaderCard title="Subcards and compact lists">
        <div className="subcard-stack">
          <Subcard
            title="Backup libraries"
            actions={
              <Button width="medium" variant="secondary" icon={<Icon name="plus" />}>
                Add backup
              </Button>
            }
          >
            <CompactTable label="Backup folders" columns={["Folder", "Status"]}>
              <tr>
                <td>Archive</td>
                <td>
                  <StatusBadge status="complete">Scanned</StatusBadge>
                </td>
              </tr>
              <tr>
                <td>Off-site copy</td>
                <td>
                  <StatusBadge status="pending">Not scanned</StatusBadge>
                </td>
              </tr>
            </CompactTable>
          </Subcard>
          <Subcard title="Empty list">
            <p className="subcard-note">No backup libraries.</p>
          </Subcard>
        </div>
      </HeaderCard>
      <Card title="Shared queue status pills">
        <div className="actions">
          {[
            "queued",
            "searching",
            "downloading",
            "validating",
            "normalizing",
            "publishing",
            "awaiting_input",
            "backup_pending",
            "complete",
            "failed",
          ].map((state) => (
            <JobStatus
              key={state}
              job={{
                id: 0,
                plan_version: 1,
                created_at: 0,
                album: null,
                album_artist: null,
                approved_at: 1,
                artwork_candidate_id: null,
                state,
              }}
            />
          ))}
        </div>
        <p>
          The same fixed-width pills appear in the queue, requests and notifications. Animation
          respects reduced motion.
        </p>
      </Card>
      <FocusHeader onSearch={() => document.getElementById("music-search")?.focus()} />
      <HomeQueue jobs={[]} onChanged={() => {}} disabled />
    </>
  );
}

export function App() {
  return (
    <AlbumSelectionProvider>
      <ConnectedApp />
    </AlbumSelectionProvider>
  );
}

function ConnectedApp() {
  const selection = useAlbumSelection();
  const { current, page: route } = useRoute();
  const settingsOpen = isSettingsRoute(current);
  const dismissSettings = () => window.location.replace(`#${route}`);
  const session = useResource(
    (signal) => request<SessionIdentity>("/api/session", { signal }),
    "session",
  );
  const setup = useResource(async (signal) => {
    const [locations, settings, settingsPolicy, librarySetup] = await Promise.all([
      request<Location[]>("/api/locations?limit=100", { signal }),
      request<SettingsVersion | null>("/api/settings", { signal }),
      request<SystemSettingsPolicy>("/api/system/settings-policy", { signal }),
      request<LibrarySetup>("/api/library/setup", { signal }),
    ]);
    return { locations, settings, settingsPolicy, librarySetup };
  }, "setup");
  const setupChecked = useRef(false);
  useEffect(() => {
    if (setupChecked.current || setup.loading || setup.error || !setup.data) return;
    setupChecked.current = true;
    const status = setup.data.librarySetup;
    if (needsLibrarySetup(status)) {
      window.location.hash = "/setup";
    }
  }, [setup.data, setup.loading, setup.error]);
  const [poll, setPoll] = useState(false);
  const local = useResource(
    (signal) => request<LocalStatus>("/api/local/status", { signal }),
    "local",
    poll ? 2000 : 15000,
  );
  const [jobPages, setJobPages] = useState([0]);
  const [decisionPages, setDecisionPages] = useState([0]);
  const decisionAfter = decisionPages.at(-1) ?? 0;
  const decisions = useResource(
    (signal) => request<StoredQuestion[]>(`/api/inbox?after=${decisionAfter}&limit=50`, { signal }),
    String(decisionAfter),
    15000,
  );
  const after = jobPages.at(-1) ?? 0;
  const jobs = useResource(
    (signal) => request<JobSummary[]>(`/api/queue?after=${after}&limit=50`, { signal }),
    String(after),
    poll ? 2000 : 15000,
  );
  useEffect(() => {
    setPoll(
      Boolean(
        local.data?.busy ||
        local.data?.worker?.unfinished ||
        jobs.data?.some((job) => activeStates.has(job.state)),
      ),
    );
  }, [local.data, jobs.data]);
  const [term, setTerm] = useState("");
  const [recent, setRecent] = useState<string[]>([]);
  const [problemsOpen, setProblemsOpen] = useState(false);
  const [utility, setUtility] = useState<"account" | "agent" | null>(null);
  useEffect(() => {
    function close(event: KeyboardEvent) {
      if (event.key === "Escape") {
        setProblemsOpen(false);
        setUtility(null);
      }
    }
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, []);
  const problems = groupJobs(jobs.data ?? []).attention;
  const problemCount = problems.length + (local.data?.last_error ? 1 : 0);
  const locationCommand = useCommand();
  const [moreLocations, setMoreLocations] = useState<{
    base: Location[] | undefined;
    rows: Location[];
    complete: boolean;
  }>({ base: undefined, rows: [], complete: false });
  const extraLocations = moreLocations.base === setup.data?.locations ? moreLocations.rows : [];
  const locations = [...(setup.data?.locations ?? []), ...extraLocations];
  const locationsComplete =
    (setup.data?.locations.length ?? 0) < 100 ||
    (moreLocations.base === setup.data?.locations && moreLocations.complete);
  const parts = route.split("/");
  const section = (parts[1] ?? "").split("?")[0] ?? "";
  const sectionTitle =
    section === "inbox"
      ? "Decisions"
      : section === "library-scans"
        ? "Library scans"
        : section
          ? section.charAt(0).toUpperCase() + section.slice(1)
          : "Home";
  const routeId = parts[2] ? decodeURIComponent(parts[2]) : "";
  const routeVersion = Number(parts[3]);
  function changed() {
    jobs.refresh();
    local.refresh();
    decisions.refresh();
  }
  function onCreated(job: number, version: number) {
    changed();
    window.location.hash = `/request/${job}/${version}`;
  }
  let page;
  if (route.startsWith("/search")) {
    const search = new URLSearchParams(route.split("?")[1]).get("q") ?? "";
    page = <Search key={search} term={search} locations={locations} />;
  } else if (parts[1] === "artist" && routeId) {
    page = <Artist key={routeId} id={routeId} locations={locations} />;
  } else if (parts[1] === "album" && routeId) {
    page = (
      <Album
        key={routeId}
        id={routeId}
        settings={setup.data?.settings}
        locations={locations}
        onCreated={onCreated}
      />
    );
  } else if (
    parts[1] === "request" &&
    Number.isSafeInteger(Number(routeId)) &&
    Number(routeId) > 0 &&
    Number.isSafeInteger(routeVersion) &&
    routeVersion > 0
  ) {
    page = (
      <RequestPage
        key={`${routeId}:${routeVersion}`}
        id={Number(routeId)}
        version={routeVersion}
        status={local.error ? undefined : local.data}
        onChanged={changed}
        audioProfiles={setup.data?.settings?.audio_profiles}
        reviewNext={selection?.items
          .filter((item) => item.plan && !item.submitted && item.plan.job_id !== Number(routeId))
          .map((item) => item.plan!)}
        onStarted={(next) => {
          const item = selection?.items.find(
            (item) => item.plan?.job_id === Number(routeId) && item.plan.version === routeVersion,
          );
          if (item) selection?.update(item.release.id, { submitted: true });
          if (next) window.location.hash = `/request/${next.job_id}/${next.version}`;
        }}
      />
    );
  } else if (route === "/queue") {
    page = (
      <>
        <header className="page-heading">
          <div>
            <h1>Your download queue</h1>
            <p>Saved server state. Cancelled requests stay out of this list.</p>
          </div>
          <Button variant="secondary" onClick={changed}>
            Refresh queue
          </Button>
        </header>
        <Alert error={jobs.error} />
        <Alert error={local.error} />
        {!local.error && local.data?.worker && <WorkerRequests status={local.data.worker} />}
        {jobs.loading && !jobs.data && <p role="status">Loading requests…</p>}
        {jobs.data && (
          <QueuePage
            jobs={jobs.data ?? []}
            onChanged={changed}
            disabled={jobs.loading || Boolean(jobs.error)}
          />
        )}
        <Pager
          busy={jobs.loading}
          previous={jobPages.length > 1 ? () => setJobPages(jobPages.slice(0, -1)) : undefined}
          next={
            jobs.data?.length === 50
              ? () => setJobPages([...jobPages, jobs.data?.at(-1)?.id ?? after])
              : undefined
          }
        />
      </>
    );
  } else if (section === "recovery") {
    const job = recoveryJobFilter(route);
    page = <Recovery key={job ?? "all"} {...(job ? { job } : {})} />;
  } else if (parts[1] === "inbox") {
    const selectedId = Number(routeId);
    page = (
      <Inbox
        questions={decisions.data ?? []}
        loading={decisions.loading}
        error={decisions.error}
        selectedId={Number.isSafeInteger(selectedId) && selectedId > 0 ? selectedId : undefined}
        refresh={changed}
        previous={
          decisionPages.length > 1 ? () => setDecisionPages(decisionPages.slice(0, -1)) : undefined
        }
        next={
          decisions.data?.length === 50
            ? () =>
                setDecisionPages([...decisionPages, decisions.data?.at(-1)?.id ?? decisionAfter])
            : undefined
        }
      />
    );
  } else if (route === "/setup") {
    page = setup.data ? (
      <SetupWizard
        setup={setup.data.librarySetup}
        locations={locations}
        readOnly={setup.data.settingsPolicy.read_only}
        loading={setup.loading}
        error={setup.error}
        onChanged={setup.refresh}
      />
    ) : (
      <Card title="Checking required setup">
        <Alert error={setup.error} />
        <p>
          Reading the primary library and shared output policy. Failed reads do not mean setup is
          missing.
        </p>
        <Button disabled={setup.loading} onClick={setup.refresh}>
          Retry setup check
        </Button>
      </Card>
    );
  } else if (route === "/library-scans") {
    page = (
      <LibraryMaintenance
        locations={locations}
        readOnly={
          (setup.data?.settingsPolicy.read_only ?? true) || setup.loading || Boolean(setup.error)
        }
        worker={local.error || local.loading ? undefined : (local.data?.worker ?? undefined)}
        onChanged={setup.refresh}
      />
    );
  } else if (route.split("?")[0] === "/library") {
    page = (
      <Library
        locations={locations}
        route={route}
        readOnly={setup.data?.settingsPolicy.read_only ?? true}
      />
    );
  } else if (route === "/components") {
    page = <ComponentLibrary />;
  } else {
    page = (
      <>
        <FocusHeader onSearch={() => document.getElementById("music-search")?.focus()} />
        {setup.data && (!setup.data.settings || locations.length === 0) && (
          <Card title="Local setup is incomplete">
            <p>
              Complete the setup wizard to select a primary library and save shared output policy.
            </p>
            <a href="#/setup">Open setup wizard</a>
          </Card>
        )}
        <Alert error={jobs.error} />
        {jobs.loading && !jobs.data ? (
          <p role="status">Reading the saved queue…</p>
        ) : jobs.data ? (
          <HomeQueue
            jobs={jobs.data ?? []}
            onChanged={changed}
            disabled={jobs.loading || Boolean(jobs.error)}
          />
        ) : null}
        <div className="home-grid">
          <HeaderCard
            title="Suggested next"
            className="suggestions"
            actions={<Badge>Coming later</Badge>}
          >
            <EmptyState title="No suggestions yet">
              A future service will fill this book with artists and albums.
            </EmptyState>
          </HeaderCard>
          <HeaderCard title="Recent searches">
            <p className="muted">From this browser session. Select one to search again.</p>
            <div className="recent-searches">
              {recent.map((value) => (
                <a
                  key={value}
                  className="button button-secondary"
                  href={`#/search?q=${encodeURIComponent(value)}`}
                >
                  {value}
                </a>
              ))}
              {!recent.length && <EmptyState title="Your searches will appear here" />}
            </div>
            <a href="#/library">Browse the indexed library</a>
          </HeaderCard>
        </div>
      </>
    );
  }
  return (
    <div className="app-shell">
      <a
        className="skip-link"
        href="#main"
        onClick={(event) => {
          event.preventDefault();
          document.getElementById("main")?.focus();
        }}
      >
        Skip to Music content
      </a>
      <header className="orbit">
        <div className="orbit-left">
          <a className="brand" href="#/" aria-label="Domus Obscura home">
            <b>DOMUS</b>
            <span>OBSCURA</span>
          </a>
          <Button
            variant="ghost"
            aria-label="Back"
            className="icon-button"
            onClick={() => window.history.back()}
          >
            <Icon name="back" />
          </Button>
          <Button
            variant="ghost"
            aria-label="Forward"
            className="icon-button"
            onClick={() => window.history.forward()}
          >
            <Icon name="forward" />
          </Button>
          <a href="#/" className="icon-button home-link" aria-label="Home">
            <Icon name="home" />
          </a>
        </div>
        <form
          role="search"
          className="orbit-search"
          onSubmit={(event) => {
            event.preventDefault();
            const value = term.trim();
            if (!value) return;
            setRecent((current) =>
              [value, ...current.filter((item) => item !== value)].slice(0, 12),
            );
            window.location.hash = `/search?q=${encodeURIComponent(value)}`;
          }}
        >
          <Icon name="search" />
          <label className="sr-only" htmlFor="music-search">
            Search MusicBrainz artists and albums
          </label>
          <input
            id="music-search"
            type="search"
            value={term}
            onChange={(event) => setTerm(event.target.value)}
            placeholder=" "
            maxLength={256}
            required
          />
          <span className="search-prompt" aria-hidden="true">
            Search an artist or album to start a download
          </span>
          {term && (
            <Button
              variant="ghost"
              className="icon-button search-clear"
              aria-label="Clear search"
              onClick={() => {
                setTerm("");
                document.getElementById("music-search")?.focus();
              }}
            >
              <Icon name="close" />
            </Button>
          )}
        </form>
        <div className="orbit-tools">
          <a
            href="#/queue"
            className="icon-button home-link"
            aria-label="Download queue"
            title="Download queue"
          >
            <Icon name="queue" />
          </a>
          <Button
            variant="ghost"
            className="icon-button notification-button"
            aria-label={
              problemCount
                ? `Open notifications, ${problemCount} need attention on this page`
                : "Open notifications"
            }
            title="Notifications"
            aria-expanded={problemsOpen}
            onClick={() => {
              setProblemsOpen(!problemsOpen);
              setUtility(null);
            }}
          >
            <Icon name="bell" />
            {problemCount > 0 && <span className="notification-dot" />}
          </Button>
          <Button
            variant="ghost"
            className="icon-button"
            aria-label="Future AI agent"
            title="Future AI agent"
            aria-expanded={utility === "agent"}
            onClick={() => {
              setUtility(utility === "agent" ? null : "agent");
              setProblemsOpen(false);
            }}
          >
            <Icon name="agent" />
          </Button>
        </div>
      </header>
      <nav className="sidebar" aria-label="Music navigation">
        <a
          href="#/"
          className={`side-link ${!["settings", "recovery", "library-scans", "setup"].includes(section) ? "selected" : ""}`}
          aria-label="Media"
          title="Media"
        >
          <Icon name="music" />
          <span className="sr-only">Media</span>
        </a>
        <a
          href="#/recovery"
          className={`side-link ${section === "recovery" ? "selected" : ""}`}
          aria-label="Recovery"
          title="Recovery"
          aria-current={section === "recovery" ? "page" : undefined}
        >
          <Icon name="refresh" />
          <span className="sr-only">Recovery</span>
        </a>
        <a
          href="#/library-scans"
          className={`side-link ${route === "/library-scans" ? "selected" : ""}`}
          aria-label="Library scans"
          title="Library scans"
          aria-current={route === "/library-scans" ? "page" : undefined}
        >
          <Icon name="scan" />
          <span className="sr-only">Library scans</span>
        </a>
        <div className="sidebar-utilities">
          <a
            className="side-link"
            href="#/components"
            aria-label="Component library"
            title="Component library"
          >
            <Icon name="inbox" />
          </a>
          <a
            className={`side-link ${settingsOpen ? "selected" : ""}`}
            href="#/settings"
            aria-label="System settings"
            title="System settings"
            aria-haspopup="dialog"
          >
            <Icon name="settings" />
          </a>
          <Button
            variant="ghost"
            className="side-link"
            aria-label="Account"
            title="Account"
            aria-expanded={utility === "account"}
            onClick={() => {
              setUtility(utility === "account" ? null : "account");
              setProblemsOpen(false);
            }}
          >
            <Icon name="account" />
          </Button>
        </div>
      </nav>
      <main id="main" tabIndex={-1}>
        <ProviderWaiting />
        <nav className="breadcrumbs" aria-label="Current location">
          <a href="#/">Media</a>
          <span aria-hidden="true"> · </span>
          <span>{sectionTitle}</span>
        </nav>
        {problemsOpen && (
          <aside className="problems-panel app-drawer" aria-label="Notifications">
            <div className="section-heading">
              <h2>Needs attention</h2>
              <Button variant="ghost" onClick={() => setProblemsOpen(false)}>
                Close
              </Button>
            </div>
            <Alert error={local.error} />
            <Alert error={jobs.error} />
            {local.data?.last_error && <p className="warning">{local.data.last_error}</p>}
            <a href="#/inbox">Open saved decisions and recovery actions</a>
            <Alert error={decisions.error} />
            {problems.length ? (
              <QueueCard
                title="Requests needing attention"
                jobs={problems}
                onChanged={changed}
                disabled={jobs.loading || Boolean(jobs.error)}
                attention
              />
            ) : (
              !local.data?.last_error && <Empty>No problems in the loaded queue page.</Empty>
            )}
          </aside>
        )}
        {utility && (
          <aside
            className="app-drawer"
            aria-label={utility === "account" ? "Account" : "Future AI agent"}
          >
            <div className="section-heading">
              <h2>{utility === "account" ? "Account" : "AI agent (preview)"}</h2>
              <Button variant="ghost" onClick={() => setUtility(null)}>
                Close
              </Button>
            </div>
            {utility === "account" ? (
              <>
                <p>Current browser identity</p>
                <p className="identifier">{session.data?.object_id ?? "Not signed in"}</p>
                <p className="muted">Shared-library policy is controlled in System settings.</p>
                <a href="#/settings" onClick={() => setUtility(null)}>
                  Open System settings
                </a>
              </>
            ) : (
              <>
                <p>No model or agent is connected.</p>
                <p className="muted">
                  This saved design placeholder does not run commands or approve downloads. Runtime
                  AI remains outside V1.
                </p>
              </>
            )}
          </aside>
        )}
        <Alert error={session.error || setup.error || local.error} />
        {session.data ? (
          page
        ) : session.loading ? (
          <p role="status">Checking the current browser session…</p>
        ) : (
          <Card title="Connect to Domus Obscura">
            <p>
              Sign in for a production host. An explicitly configured development host returns its
              fixed identity from the session endpoint.
            </p>
            <a className="button button-default" href="/auth/login">
              Sign in with Microsoft
            </a>
          </Card>
        )}
        {session.data && (
          <AlbumSelection
            settings={setup.data?.settings}
            locations={locations}
            blocked={setup.loading || Boolean(setup.error) || selection?.busy === true}
            onPrepared={onCreated}
          />
        )}
        {session.data && !locationsComplete && (
          <div className="actions">
            <Button
              variant="secondary"
              disabled={locationCommand.busy || setup.loading || Boolean(setup.error)}
              onClick={() =>
                void locationCommand.run(async () => {
                  const rows = await request<Location[]>(
                    `/api/locations?after=${locations.at(-1)?.id ?? 0}&limit=100`,
                  );
                  setMoreLocations({
                    base: setup.data?.locations,
                    rows: [...extraLocations, ...rows],
                    complete: rows.length < 100,
                  });
                })
              }
            >
              Load more library roots
            </Button>
            <Alert error={locationCommand.error} />
          </div>
        )}
        <footer>Music only · Local collection · No automatic downloads</footer>
      </main>
      {session.data && settingsOpen && (
        <SettingsDialog onDismiss={dismissSettings}>
          <Settings
            key={current}
            settings={setup.data?.settings}
            locations={locations}
            policy={setup.data?.settingsPolicy}
            onSaved={() => setup.refresh()}
            error={setup.error}
            loading={setup.loading}
            onReload={setup.refresh}
            librarySetup={setup.data?.librarySetup}
            onDismiss={dismissSettings}
          />
        </SettingsDialog>
      )}
    </div>
  );
}
