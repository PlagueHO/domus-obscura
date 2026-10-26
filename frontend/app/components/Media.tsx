import { useState, type ReactNode } from "react";
import type { JobSummary } from "../../src/generated/JobSummary";
import type { CatalogArtist } from "../../src/generated/CatalogArtist";
import type { CatalogReleaseSummary } from "../../src/generated/CatalogReleaseSummary";
import type { QueueSelection } from "../../src/generated/QueueSelection";
import { request } from "../api";
import { useCommand } from "../hooks";
import { visibleJobs } from "../state";
import { Alert, Button, Cover, Empty } from "./ui";

export type IconName =
  | "home"
  | "search"
  | "music"
  | "queue"
  | "inbox"
  | "settings"
  | "back"
  | "forward"
  | "bell"
  | "account"
  | "close"
  | "agent";

export function Icon({ name }: { name: IconName }) {
  const paths: Record<IconName, string> = {
    home: "m3 10 9-7 9 7v10H4V10m5 10v-7h6v7",
    search: "m16 16 5 5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0",
    music: "M9 18V5l12-2v13M9 7l12-2M9 18c0 2-6 4-6 1s6-4 6-1Zm12-2c0 2-6 4-6 1s6-4 6-1Z",
    queue: "M8 6h13M8 12h13M8 18h13M3 6h1m-1 6h1m-1 6h1",
    inbox: "M4 4h16v16H4V4m0 9h5l1 3h4l1-3h5",
    settings:
      "M12 3v3m0 12v3M3 12h3m12 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0",
    back: "m14 5-7 7 7 7",
    forward: "m10 5 7 7-7 7",
    bell: "M5 17h14l-2-3V9a5 5 0 0 0-10 0v5l-2 3m5 3h4",
    account: "M16 7a4 4 0 1 1-8 0 4 4 0 0 1 8 0M4 21v-3c0-5 16-5 16 0v3",
    close: "m6 6 12 12M6 18 18 6",
    agent: "m9 3 2 6 6 2-6 2-2 6-2-6-6-2 6-2 2-6m10 11 1 3 3 1-3 1-1 3-1-3-3-1 3-1 1-3",
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

export function FocusHeader({ onSearch }: { onSearch: () => void }) {
  return (
    <header className="focus-header">
      <div>
        <h1>Find music to download</h1>
        <p>Search for an artist or album to start a download.</p>
        <p className="focus-flow">Search · Choose an edition · Download</p>
        <Button onClick={onSearch}>
          <Icon name="search" />
          Start a search
        </Button>
      </div>
      <div className="focus-art" aria-hidden="true">
        {["planet", "green", "blue", "planet"].map((variant, index) => (
          <i key={index}>
            <svg viewBox="0 0 240 240">
              <rect
                width="240"
                height="240"
                fill={variant === "green" ? "#173f35" : variant === "blue" ? "#0f2d46" : "#16213e"}
              />
              {variant === "planet" ? (
                <>
                  <circle cx="120" cy="120" r="52" fill="none" stroke="#e582b8" strokeWidth="26" />
                  <ellipse
                    cx="120"
                    cy="120"
                    rx="96"
                    ry="26"
                    transform="rotate(-22 120 120)"
                    fill="none"
                    stroke="#f4e9b5"
                    strokeWidth="5"
                  />
                </>
              ) : (
                <>
                  <path
                    d="M0 190 90 60l60 90 30-40 60 80Z"
                    fill={variant === "green" ? "#a5cf72" : "#52b8e0"}
                  />
                  <circle
                    cx="170"
                    cy="62"
                    r="20"
                    fill={variant === "green" ? "#e8f2c8" : "#e2f6ff"}
                  />
                </>
              )}
            </svg>
          </i>
        ))}
      </div>
    </header>
  );
}

export function ArtistCard({ artist }: { artist: CatalogArtist }) {
  return (
    <a className="media-card artist-card" href={`#/artist/${encodeURIComponent(artist.id)}`}>
      <Cover title={artist.name} />
      <span className="card-text">
        <strong>{artist.name}</strong>
        <span>
          {[artist.country, artist.kind, artist.disambiguation].filter(Boolean).join(" · ") ||
            "Artist"}
        </span>
      </span>
    </a>
  );
}

export function AlbumCard({ release }: { release: CatalogReleaseSummary }) {
  return (
    <a className="media-card album-card" href={`#/album/${encodeURIComponent(release.id)}`}>
      <Cover title={release.title} />
      <strong title={release.title}>{release.title}</strong>
      <span title={release.artist}>{release.artist}</span>
      <small title={release.disambiguation ?? undefined}>
        {[release.date, release.country, release.disambiguation].filter(Boolean).join(" · ") ||
          "Exact edition"}
      </small>
    </a>
  );
}

const statuses = {
  attention: {
    label: "Needs your input",
    path: "M12 7v6m0 3v1M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0",
  },
  queued: { label: "Queued", path: "M12 7v5l3 2M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0" },
  downloading: { label: "Downloading", path: "M12 3v13m-5-5 5 5 5-5M5 21h14" },
  verifying: { label: "Verifying", path: "M21 12a9 9 0 1 1-6-8.5" },
  publishing: { label: "Publishing", path: "M12 21V8m-5 5 5-5 5 5M5 3h14" },
  finished: { label: "In library", path: "m5 12 4 4L19 6" },
  failed: { label: "Failed", path: "m6 6 12 12M6 18 18 6" },
  searching: { label: "Searching", path: "m16 16 5 5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0" },
  backup: { label: "Backup pending", path: "M12 7v6m0 3v1M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0" },
} as const;

export function jobPresentation(job: JobSummary): keyof typeof statuses {
  switch (job.state) {
    case "awaiting_input":
      return "attention";
    case "failed":
      return "failed";
    case "backup_pending":
      return "backup";
    case "complete":
      return "finished";
    case "downloading":
      return "downloading";
    case "validating":
    case "normalizing":
      return "verifying";
    case "publishing":
      return "publishing";
    case "searching":
      return "searching";
    default:
      return "attention";
    case "queued":
      return job.approved_at === null ? "attention" : "queued";
  }
}

export function JobStatus({ job }: { job: JobSummary }) {
  const state = jobPresentation(job);
  const { label, path } = statuses[state];
  return (
    <span className={`status-pill status-${state}`}>
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d={path} />
      </svg>
      {label}
    </span>
  );
}

export function groupJobs(jobs: JobSummary[]) {
  const visible = visibleJobs(jobs);
  return {
    attention: visible.filter((job) =>
      ["attention", "failed", "backup"].includes(jobPresentation(job)),
    ),
    active: visible.filter(
      (job) => !["attention", "failed", "backup", "finished"].includes(jobPresentation(job)),
    ),
    finished: visible.filter((job) => job.state === "complete"),
  };
}

export function QueueCover({ job }: { job: JobSummary }) {
  const [failed, setFailed] = useState(false);
  const title = job.album ?? `Request ${job.id}`;
  if (job.artwork_candidate_id === null) return <Cover title={title} />;
  if (failed)
    return (
      <span className="queue-cover-error" role="status">
        Cover unavailable
      </span>
    );
  return (
    <img
      className="queue-cover-image"
      src={`/api/jobs/${job.id}/plans/${job.plan_version}/artwork/${job.artwork_candidate_id}/image`}
      alt={`Approved cover for ${title}`}
      width="56"
      height="56"
      loading="lazy"
      onError={() => setFailed(true)}
    />
  );
}

function JobRow({
  job,
  onChanged,
  disabled,
}: {
  job: JobSummary;
  onChanged: () => void;
  disabled: boolean;
}) {
  const href = `#/request/${job.id}/${job.plan_version}`;
  return (
    <li className="queue-row">
      <a href={href} className="queue-cover" aria-label={`Open request ${job.id}`}>
        <QueueCover key={`${job.id}:${job.plan_version}:${job.artwork_candidate_id}`} job={job} />
      </a>
      <div className="queue-title">
        <a href={href}>
          <strong>{job.album ?? `Album request #${job.id}`}</strong>
        </a>
        <small>
          {job.album_artist ?? "No frozen edition"} · Plan {job.plan_version}
        </small>
      </div>
      <JobStatus job={job} />
      <a
        className="button button-secondary queue-action"
        href={job.state === "awaiting_input" ? "#/inbox" : href}
      >
        {["attention", "failed", "backup"].includes(jobPresentation(job)) ? "Resolve" : "Open"}
      </a>
      <CancelJob job={job} disabled={disabled} onChanged={onChanged} />
      {job.state === "complete" && (
        <DismissFinished jobs={[job]} disabled={disabled} onChanged={onChanged} />
      )}
    </li>
  );
}

export function CancelJob({
  job,
  disabled,
  onChanged,
}: {
  job: JobSummary;
  disabled: boolean;
  onChanged: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const command = useCommand();
  if (
    !["queued", "searching", "downloading", "validating", "normalizing", "awaiting_input"].includes(
      job.state,
    )
  )
    return null;
  return (
    <>
      <Button
        variant="ghost"
        className="icon-button"
        aria-label={`Cancel request ${job.id}`}
        disabled={disabled || command.busy || submitted}
        aria-expanded={confirm}
        onClick={() => setConfirm(!confirm)}
      >
        <Icon name="close" />
      </Button>
      {confirm && (
        <div className="queue-confirm">
          <p>
            Cancel this download? Files already saved stay where they are. An active transfer must
            first reach a safe boundary.
          </p>
          <div className="actions">
            <Button
              variant="secondary"
              disabled={command.busy || submitted}
              onClick={() => setConfirm(false)}
            >
              Keep it
            </Button>
            <Button
              variant="destructive"
              disabled={disabled || command.busy || submitted}
              onClick={() =>
                void command.run(async () => {
                  await request(`/api/jobs/${job.id}/plans/${job.plan_version}/controls`, {
                    body: { action: "cancel" },
                  });
                  setSubmitted(true);
                  onChanged();
                })
              }
            >
              Cancel download
            </Button>
          </div>
          {submitted && (
            <p role="status">
              Cancellation requested. The row stays until the server confirms cancellation.
            </p>
          )}
          <Alert error={command.error} />
        </div>
      )}
    </>
  );
}

export function finishedSelection(jobs: JobSummary[]): QueueSelection[] {
  return jobs
    .filter((job) => job.state === "complete")
    .map((job) => ({ job_id: job.id, plan_version: job.plan_version }));
}

function DismissFinished({
  jobs,
  disabled,
  onChanged,
  bulk = false,
}: {
  jobs: JobSummary[];
  disabled: boolean;
  onChanged: () => void;
  bulk?: boolean;
}) {
  const command = useCommand();
  const [submitted, setSubmitted] = useState(false);
  return (
    <>
      <Button
        variant="ghost"
        className={bulk ? "queue-dismiss-all" : "queue-dismiss"}
        disabled={disabled || command.busy || submitted}
        aria-label={
          bulk ? "Clear finished requests on this page" : `Remove request ${jobs[0]?.id} from queue`
        }
        onClick={() =>
          void command.run(async () => {
            await request("/api/queue/dismiss", { body: finishedSelection(jobs) });
            setSubmitted(true);
            onChanged();
          })
        }
      >
        {bulk ? "Clear finished on this page" : "Remove"}
      </Button>
      {command.error && (
        <div className="queue-confirm">
          <Alert error={command.error} />
        </div>
      )}
    </>
  );
}

export function QueueCard({
  title,
  jobs,
  onChanged,
  disabled = false,
  children,
  attention = false,
}: {
  title: string;
  jobs: JobSummary[];
  onChanged: () => void;
  disabled?: boolean;
  children?: ReactNode;
  attention?: boolean;
}) {
  return (
    <section className={`design-card queue-card ${attention ? "attention-card" : ""}`}>
      <header className="design-card-header">
        <h2>{title}</h2>
        <span className="muted">{jobs.length} on this page</span>
      </header>
      <div className="design-card-body">
        <ul className="queue-rows">
          {jobs.map((job) => (
            <JobRow
              key={`${job.id}:${job.plan_version}:${job.state}`}
              job={job}
              onChanged={onChanged}
              disabled={disabled}
            />
          ))}
        </ul>
        {children}
      </div>
    </section>
  );
}

export function QueuePage({
  jobs,
  onChanged,
  disabled = false,
}: {
  jobs: JobSummary[];
  onChanged: () => void;
  disabled?: boolean;
}) {
  const groups = groupJobs(jobs);
  const [all, setAll] = useState(false);
  return (
    <div className="queue-page">
      {!!groups.attention.length && (
        <QueueCard
          title="Needs your input"
          jobs={groups.attention}
          onChanged={onChanged}
          disabled={disabled}
          attention
        />
      )}
      {!!groups.active.length && (
        <QueueCard
          title="In progress"
          jobs={groups.active}
          onChanged={onChanged}
          disabled={disabled}
        />
      )}
      {!!groups.finished.length && (
        <QueueCard
          title="Finished"
          jobs={all ? groups.finished : groups.finished.slice(0, 5)}
          onChanged={onChanged}
          disabled={disabled}
        >
          {groups.finished.length > 5 && (
            <Button variant="ghost" aria-expanded={all} onClick={() => setAll(!all)}>
              {all ? "Show fewer" : `Show all ${groups.finished.length}`}
            </Button>
          )}
          <DismissFinished
            key={groups.finished.map((job) => `${job.id}:${job.plan_version}`).join(",")}
            jobs={groups.finished}
            disabled={disabled}
            onChanged={onChanged}
            bulk
          />
          <p className="queue-note">
            Finished requests leave this list after 7 days, or when you remove them. Clearing this
            page also removes finished rows behind Show all. Library files and recovery records stay
            in place.
          </p>
        </QueueCard>
      )}
      {!visibleJobs(jobs).length && (
        <Empty>No requests on this queue page. Search for an artist or album to start.</Empty>
      )}
    </div>
  );
}

export function HomeQueue({
  jobs,
  onChanged,
  disabled = false,
}: {
  jobs: JobSummary[];
  onChanged: () => void;
  disabled?: boolean;
}) {
  const groups = groupJobs(jobs);
  const [expanded, setExpanded] = useState<boolean>();
  const open = expanded ?? groups.attention.length > 0;
  return (
    <section
      className={`design-card home-queue ${groups.attention.length ? "attention-card" : ""}`}
    >
      <header className="design-card-header">
        <h2>Download queue</h2>
        <span className="queue-summary">
          {groups.attention.length
            ? `${groups.attention.length} need your input`
            : `${groups.active.length} in progress · ${groups.finished.length} finished`}
        </span>
        <Button variant="ghost" aria-expanded={open} onClick={() => setExpanded(!open)}>
          {open ? "Hide" : "Show"}
        </Button>
        <a href="#/queue">Open queue</a>
      </header>
      {open && (
        <div className="design-card-body">
          <ul className="queue-rows">
            {[...groups.attention, ...groups.active].slice(0, 5).map((job) => (
              <JobRow
                key={`${job.id}:${job.plan_version}:${job.state}`}
                job={job}
                onChanged={onChanged}
                disabled={disabled}
              />
            ))}
          </ul>
          {!groups.attention.length && !groups.active.length && (
            <Empty>No active requests on the loaded queue page.</Empty>
          )}
        </div>
      )}
    </section>
  );
}
