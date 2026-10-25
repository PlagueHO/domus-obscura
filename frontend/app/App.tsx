import { useEffect, useState } from "react";
import type { SessionIdentity } from "../src/generated/SessionIdentity";
import type { Location } from "../src/generated/Location";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { JobSummary } from "../src/generated/JobSummary";
import { request, type LocalStatus } from "./api";
import { useResource } from "./hooks";
import { activeStates, stateLabel, visibleJobs } from "./state";
import { Alert, Badge, Button, Card, Check, Cover, Empty, Pager } from "./components/ui";
import { Album, Catalog, Settings } from "./Catalog";
import { RequestPage } from "./Request";
import { Library } from "./Library";

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

function Icon({
  name,
}: {
  name: "home" | "search" | "music" | "queue" | "settings" | "back" | "forward";
}) {
  const paths = {
    home: "m3 10 9-7 9 7v10H4V10m5 10v-7h6v7",
    search: "m16 16 5 5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
    music: "M9 18V5l12-2v13M9 7l12-2M9 18c0 2-6 4-6 1s6-4 6-1Zm12-2c0 2-6 4-6 1s6-4 6-1Z",
    queue: "M8 6h13M8 12h13M8 18h13M3 6h1m-1 6h1m-1 6h1",
    settings:
      "M12 3v3m0 12v3M3 12h3m12 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0",
    back: "m14 5-7 7 7 7",
    forward: "m10 5 7 7-7 7",
  };
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={paths[name]} />
    </svg>
  );
}

function JobRows({ jobs }: { jobs: JobSummary[] }) {
  const visible = visibleJobs(jobs);
  return visible.length ? (
    <div className="job-list">
      {visible.map((job) => (
        <a
          className="list-row job-row"
          key={job.id}
          href={`#/request/${job.id}/${job.plan_version}`}
        >
          <span className="request-number">#{job.id}</span>
          <span>
            <strong>Album request #{job.id}</strong>
            <small>
              Plan {job.plan_version} · {new Date(job.created_at * 1000).toLocaleString()}
            </small>
          </span>
          <Badge>{stateLabel(job.state)}</Badge>
        </a>
      ))}
    </div>
  ) : (
    <Empty>No visible requests on this page. Search Music to prepare an exact album request.</Empty>
  );
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
    const [locations, settings] = await Promise.all([
      request<Location[]>("/api/locations?limit=100", { signal }),
      request<SettingsVersion | null>("/api/settings", { signal }),
    ]);
    return { locations, settings };
  }, "setup");
  const [poll, setPoll] = useState(false);
  const local = useResource(
    (signal) => request<LocalStatus>("/api/local/status", { signal }),
    "local",
    poll ? 2000 : 0,
  );
  const [jobPages, setJobPages] = useState([0]);
  const after = jobPages.at(-1) ?? 0;
  const jobs = useResource(
    (signal) => request<JobSummary[]>(`/api/jobs?after=${after}&limit=50`, { signal }),
    String(after),
    poll ? 2000 : 0,
  );
  useEffect(() => {
    setPoll(Boolean(local.data?.busy || jobs.data?.some((job) => activeStates.has(job.state))));
  }, [local.data, jobs.data]);
  const [term, setTerm] = useState("");
  const [problemsOpen, setProblemsOpen] = useState(false);
  const problems =
    jobs.data?.filter((job) =>
      ["failed", "awaiting_input", "backup_pending"].includes(job.state),
    ) ?? [];
  const problemCount = problems.length + (local.data?.last_error ? 1 : 0);
  const locations = setup.data?.locations ?? [];
  const parts = route.split("/");
  const routeId = parts[2] ? decodeURIComponent(parts[2]) : "";
  const routeVersion = Number(parts[3]);
  function changed() {
    jobs.refresh();
    local.refresh();
  }
  function onCreated(job: number, version: number) {
    changed();
    window.location.hash = `/request/${job}/${version}`;
  }
  let page;
  if (route.startsWith("/search")) {
    const search = new URLSearchParams(route.split("?")[1]).get("q") ?? "";
    page = <Catalog key={search} term={search} />;
  } else if (parts[1] === "artist" && routeId) {
    page = <Catalog key={routeId} term="" artistId={routeId} />;
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
        status={local.data}
        onChanged={changed}
      />
    );
  } else if (route === "/queue") {
    page = (
      <>
        <header className="hero request-hero">
          <div>
            <h1>Your download queue</h1>
            <p>Saved server state. Cancelled requests stay out of this list.</p>
          </div>
          <Button variant="secondary" onClick={changed}>
            Refresh queue
          </Button>
        </header>
        <Alert error={jobs.error} />
        {jobs.loading && !jobs.data && <p role="status">Loading requests…</p>}
        <JobRows jobs={jobs.data ?? []} />
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
  } else if (route === "/settings") {
    page = <Settings settings={setup.data?.settings} locations={locations} />;
  } else if (route === "/library") {
    page = <Library locations={locations} />;
  } else if (route === "/components") {
    page = <ComponentLibrary />;
  } else {
    page = (
      <>
        <header className="hero home-hero">
          <div>
            <h1>A place for your Music.</h1>
            <p>
              Find the exact edition. Check the source and cover. Keep your collection on your own
              terms.
            </p>
            <a className="button button-default" href="#/search?q=">
              Explore Music
            </a>
          </div>
        </header>
        {setup.data && (!setup.data.settings || locations.length === 0) && (
          <Card title="Local setup is incomplete">
            <p>
              Register library roots and output settings with the local setup command. The app will
              not use sample paths or default approval.
            </p>
            <a href="#/settings">Inspect your setup</a>
          </Card>
        )}
        <div className="section-heading">
          <h2>Your requests</h2>
          <a href="#/queue">Open queue</a>
        </div>
        <Alert error={jobs.error} />
        <JobRows jobs={(jobs.data ?? []).slice(0, 8)} />
        <section className="home-guidance">
          <h2>One edition. Every detail.</h2>
          <p>
            MusicBrainz supplies the release and ordered tracks. The frozen plan supplies output
            names. Your approvals stay separate from provider metadata and actual download
            verification.
          </p>
        </section>
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
            window.location.hash = `/search?q=${encodeURIComponent(term.trim())}`;
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
            placeholder="Search artists, albums, exact editions…"
            maxLength={256}
          />
          <Button variant="ghost" type="submit">
            Search
          </Button>
        </form>
        <div className="orbit-tools">
          <Button
            variant="ghost"
            aria-expanded={problemsOpen}
            onClick={() => setProblemsOpen(!problemsOpen)}
          >
            Problems{problemCount > 0 && <span className="count">{problemCount}</span>}
          </Button>
          <span className="identity" title={session.data?.object_id}>
            {session.data?.object_id ?? (session.loading ? "Connecting…" : "Not signed in")}
          </span>
        </div>
      </header>
      <nav className="sidebar" aria-label="Music navigation">
        {(
          [
            ["/", "home", "Home"],
            ["/search?q=", "search", "Search"],
            ["/library", "music", "Library"],
            ["/queue", "queue", "Queue"],
            ["/settings", "settings", "Setup"],
          ] as const
        ).map(([path, icon, title]) => (
          <a
            key={path}
            href={`#${path}`}
            className={`side-link ${route === path || (path.startsWith("/search") && route.startsWith("/search")) ? "selected" : ""}`}
            aria-current={route === path ? "page" : undefined}
          >
            <Icon name={icon} />
            <span>{title}</span>
          </a>
        ))}
        <a className="side-link components-link" href="#/components">
          UI
        </a>
      </nav>
      <main id="main" tabIndex={-1}>
        {problemsOpen && (
          <section className="problems-panel" aria-label="Problems requiring attention">
            <div className="section-heading">
              <h2>Problems</h2>
              <Button variant="ghost" onClick={() => setProblemsOpen(false)}>
                Close
              </Button>
            </div>
            <Alert error={local.error} />
            {local.data?.last_error && <p className="warning">{local.data.last_error}</p>}
            {problems.length ? (
              <JobRows jobs={problems} />
            ) : (
              !local.data?.last_error && <Empty>No problems in the loaded queue page.</Empty>
            )}
          </section>
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
