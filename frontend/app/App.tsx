import { useEffect, useState } from "react";
import type { SessionIdentity } from "../src/generated/SessionIdentity";
import type { Location } from "../src/generated/Location";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { SystemSettingsPolicy } from "../src/generated/SystemSettingsPolicy";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredQuestion } from "../src/generated/StoredQuestion";
import { request, type LocalStatus } from "./api";
import { WorkerRequests } from "./WorkerRequests";
import { useResource } from "./hooks";
import { activeStates } from "./state";
import { Alert, Badge, Button, Card, Check, Cover, Empty, Pager } from "./components/ui";
import { Album, Artist, Settings } from "./Catalog";
import { RequestPage } from "./Request";
import { Library } from "./Library";
import { Inbox } from "./Inbox";
import { Search } from "./Search";
import {
  FocusHeader,
  HomeQueue,
  Icon,
  JobStatus,
  QueueCard,
  QueuePage,
  groupJobs,
} from "./components/Media";

function useRoute() {
  const [route, setRoute] = useState(window.location.hash.slice(1) || "/");
  useEffect(() => {
    function change() {
      setRoute(window.location.hash.slice(1) || "/");
      window.scrollTo(0, 0);
    }
    window.addEventListener("hashchange", change);
    return () => window.removeEventListener("hashchange", change);
  }, []);
  return route;
}

function ComponentLibrary() {
  const [checked, setChecked] = useState(false);
  return (
    <>
      <header className="hero">
        <div>
          <h1>Component library</h1>
          <p>Shared production components. This page makes no service commands.</p>
        </div>
      </header>
      <Card title="Actions and input states">
        <div className="actions">
          <Button>Primary action</Button>
          <Button variant="secondary">Secondary action</Button>
          <Button variant="destructive">Safe cancel</Button>
          <Button disabled>Unavailable action</Button>
        </div>
        <Check checked={checked} onChange={setChecked}>
          Keyboard-accessible explicit acknowledgement
        </Check>
      </Card>
      <Card title="Status and empty states">
        <Badge>Pending review</Badge>
        <Empty>No records are loaded in this component example.</Empty>
      </Card>
      <Card title="Artwork unavailable">
        <div className="component-cover">
          <Cover title="Component example" />
        </div>
        <p>Missing artwork stays explicit. It never becomes a fabricated album cover.</p>
      </Card>
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
  const route = useRoute();
  const session = useResource(
    (signal) => request<SessionIdentity>("/api/session", { signal }),
    "session",
  );
  const setup = useResource(async (signal) => {
    const [locations, settings, settingsPolicy] = await Promise.all([
      request<Location[]>("/api/locations?limit=100", { signal }),
      request<SettingsVersion | null>("/api/settings", { signal }),
      request<SystemSettingsPolicy>("/api/system/settings-policy", { signal }),
    ]);
    return { locations, settings, settingsPolicy };
  }, "setup");
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
  const locations = setup.data?.locations ?? [];
  const parts = route.split("/");
  const section = (parts[1] ?? "").split("?")[0] ?? "";
  const sectionTitle =
    section === "inbox"
      ? "Decisions"
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
    page = <Search key={search} term={search} />;
  } else if (parts[1] === "artist" && routeId) {
    page = <Artist key={routeId} id={routeId} />;
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
  } else if (route === "/settings") {
    page = (
      <Settings
        settings={setup.data?.settings}
        locations={locations}
        policy={setup.data?.settingsPolicy}
        onSaved={() => setup.refresh()}
        error={setup.error}
        loading={setup.loading}
        onReload={setup.refresh}
      />
    );
  } else if (route === "/library") {
    page = <Library locations={locations} />;
  } else if (route === "/components") {
    page = <ComponentLibrary />;
  } else {
    page = (
      <>
        <FocusHeader onSearch={() => document.getElementById("music-search")?.focus()} />
        {setup.data && (!setup.data.settings || locations.length === 0) && (
          <Card title="Local setup is incomplete">
            <p>
              Register library roots in System settings. Initialize output settings with the local
              setup command if they are missing. The app will not use sample paths or default
              approval.
            </p>
            <a href="#/settings">Inspect system settings</a>
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
          <section className="design-card suggestions">
            <header className="design-card-header">
              <h2>Suggested next</h2>
              <Badge>Coming later</Badge>
            </header>
            <div className="design-card-body">
              <Empty>
                Suggestions are not available yet. A future service will fill this book with artists
                and albums.
              </Empty>
            </div>
          </section>
          <section className="design-card">
            <header className="design-card-header">
              <h2>Recent searches</h2>
            </header>
            <div className="design-card-body">
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
                {!recent.length && <Empty>Your searches will appear here.</Empty>}
              </div>
              <a href="#/library">Browse the indexed library</a>
            </div>
          </section>
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
          className={`side-link ${route !== "/settings" ? "selected" : ""}`}
          aria-label="Media"
          title="Media"
        >
          <Icon name="music" />
          <span className="sr-only">Media</span>
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
            className={`side-link ${route === "/settings" ? "selected" : ""}`}
            href="#/settings"
            aria-label="System settings"
            title="System settings"
            aria-current={route === "/settings" ? "page" : undefined}
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
        <footer>Music only · Local collection · No automatic downloads</footer>
      </main>
    </div>
  );
}
