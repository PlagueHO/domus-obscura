import { useEffect, useState } from "react";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import type { TransferFile } from "../src/generated/TransferFile";
import type { StartLocalDownload } from "../src/generated/StartLocalDownload";
import type { QuestionDetails } from "../src/generated/QuestionDetails";
import { OutputPreview } from "../src/OutputPreview";
import { request, type LocalStatus } from "./api";
import { useCommand, useResource } from "./hooks";
import { activeStates, canStart } from "./state";
import { Alert, Badge, Button, Card, Empty, Pager } from "./components/ui";
import { SourceReview } from "./SourceReview";
import { ArtworkReview } from "./ArtworkReview";
import { CancelJob, JobStatus } from "./components/Media";
import { RequestHistory } from "./RequestHistory";
import { PublicationRecovery } from "./PublicationRecovery";

export function RequestProgress({
  planApproved,
  sourceApproved,
  coverApproved,
  inputsReceived,
}: {
  planApproved: boolean;
  sourceApproved: boolean;
  coverApproved: boolean;
  inputsReceived: boolean;
}) {
  return (
    <ol className="request-progress" aria-label="Separate request approvals">
      <li>
        <strong>Edition and outputs</strong>
        <Badge>{planApproved ? "Plan approved" : "Review the frozen plan"}</Badge>
      </li>
      <li>
        <strong>Source</strong>
        <Badge>
          {inputsReceived
            ? "Received inputs retained"
            : sourceApproved
              ? "Acknowledgement saved"
              : "Choose and acknowledge exact source"}
        </Badge>
      </li>
      <li>
        <strong>Cover</strong>
        <Badge>{coverApproved ? "Exact JPEG approved" : "Choose and approve exact preview"}</Badge>
      </li>
    </ol>
  );
}

export function RequestPage({
  id,
  version,
  status,
  onChanged,
}: {
  id: number;
  version: number;
  status: LocalStatus | undefined;
  onChanged: () => void;
}) {
  const base = `/api/jobs/${id}/plans/${version}`;
  const [poll, setPoll] = useState(false);
  const [sourcePages, setSourcePages] = useState([0]);
  const [artworkPages, setArtworkPages] = useState([0]);
  const sourceAfter = sourcePages.at(-1) ?? 0;
  const artworkAfter = artworkPages.at(-1) ?? 0;
  const job = useResource(
    async (signal) => {
      const rows = await request<JobSummary[]>(`/api/jobs?after=${id - 1}&limit=1`, { signal });
      const item = rows.find((item) => item.id === id);
      if (!item) throw new Error("This request does not exist.");
      return item;
    },
    String(id),
    poll ? 2000 : 0,
  );
  useEffect(() => {
    setPoll(Boolean(status?.busy || (job.data && activeStates.has(job.data.state))));
  }, [status?.busy, job.data]);
  const plan = useResource((signal) => request<StoredPlan>(base, { signal }), base);
  const sources = useResource(
    (signal) =>
      request<SourceCandidate[]>(`${base}/source-candidates?after=${sourceAfter}&limit=50`, {
        signal,
      }),
    `${base}:${sourceAfter}`,
  );
  const artwork = useResource(
    (signal) =>
      request<ArtworkCandidate[]>(`${base}/artwork?after=${artworkAfter}&limit=50`, { signal }),
    `${base}:${artworkAfter}`,
  );
  const receipts = useResource(
    async (signal) => {
      const [publication, transfers] = await Promise.all([
        request<PublicationDestination[]>(`${base}/publication`, { signal }),
        request<TransferFile[]>(`${base}/transfers?limit=100`, { signal }),
      ]);
      return { publication, transfers };
    },
    `${base}:${job.data?.state ?? ""}`,
    poll ? 2000 : 0,
  );
  const [sourceId, setSourceId] = useState<number>();
  const [coverId, setCoverId] = useState<number>();
  const [submitted, setSubmitted] = useState(false);
  const [notice, setNotice] = useState("");
  const [historyRevision, setHistoryRevision] = useState(0);
  const [recoveryRevision, setRecoveryRevision] = useState(0);
  const command = useCommand();
  const stored = plan.data;
  const current = job.data;
  const decision = useResource(
    async (signal) =>
      current?.state === "awaiting_input"
        ? request<QuestionDetails | null>(`${base}/decision`, { signal })
        : null,
    `${base}:${current?.state ?? ""}`,
    poll ? 2000 : 0,
  );
  const source =
    sources.data?.find((item) => item.id === sourceId) ??
    (sourceId === undefined
      ? (sources.data?.find((item) => item.approved_at !== null) ?? sources.data?.[0])
      : undefined);
  const cover =
    artwork.data?.find((item) => item.id === coverId) ??
    (coverId === undefined
      ? (artwork.data?.find((item) => item.approved_at !== null) ?? artwork.data?.[0])
      : undefined);
  const allReceived = Boolean(
    receipts.data &&
    stored &&
    receipts.data.transfers.length === stored.plan.edition.tracks.length &&
    receipts.data.transfers.length > 0 &&
    receipts.data.transfers.every((file) => file.state === "received"),
  );
  function refresh() {
    setHistoryRevision((value) => value + 1);
    job.refresh();
    plan.refresh();
    sources.refresh();
    artwork.refresh();
    receipts.refresh();
    decision.refresh();
    onChanged();
  }
  function selectSource(id: number) {
    setSourceId(id);
    if (!sources.data?.some((item) => item.id === id)) setSourcePages([Math.max(0, id - 1)]);
  }
  function selectCover(id: number) {
    setCoverId(id);
    if (!artwork.data?.some((item) => item.id === id)) setArtworkPages([Math.max(0, id - 1)]);
  }
  const stale = Boolean(current && current.plan_version !== version);
  const readsBlocked = Boolean(
    job.loading ||
    plan.loading ||
    sources.loading ||
    artwork.loading ||
    receipts.loading ||
    job.error ||
    plan.error ||
    sources.error ||
    artwork.error ||
    receipts.error,
  );
  const editable = Boolean(
    stored?.approved_at !== null && stored && current?.state === "queued" && !submitted && !stale,
  );
  const coverRecovery = Boolean(
    stored &&
    stored.approved_at !== null &&
    !stale &&
    decision.data?.current &&
    decision.data.recovery.kind === "artwork" &&
    decision.data.question.answer === null &&
    current?.state === "awaiting_input" &&
    !decision.loading &&
    !decision.error,
  );
  const canRun = Boolean(
    current &&
    stored &&
    canStart(current, stored, source, cover, status, true, allReceived) &&
    !readsBlocked &&
    !submitted &&
    !command.busy,
  );
  return (
    <>
      <header className="hero request-hero">
        <div>
          <h1>{stored?.plan.edition.album ?? `Request #${id}`}</h1>
          <p>
            {stored?.plan.edition.album_artist} · Request #{id} · Plan {version}
          </p>
          {current && <JobStatus job={current} />}
        </div>
        <Button
          variant="secondary"
          disabled={command.busy || status?.busy}
          onClick={() => {
            setRecoveryRevision((value) => value + 1);
            refresh();
          }}
        >
          Refresh request
        </Button>
      </header>
      <Alert
        error={
          job.error ||
          plan.error ||
          sources.error ||
          artwork.error ||
          receipts.error ||
          decision.error
        }
      />
      <Alert error={command.error} />
      {(job.loading || plan.loading) && !stored && <p role="status">Loading saved request…</p>}
      {stale && (
        <p className="warning">
          This plan is no longer current.{" "}
          <a href={`#/request/${id}/${current?.plan_version}`}>Open the current plan.</a>
        </p>
      )}
      {stored && current && (
        <fieldset className="review-fields" disabled={readsBlocked || stale}>
          <Card
            title={
              current.state === "queued" || current.state === "awaiting_input"
                ? "What you need to do"
                : "Request status"
            }
            className="request-review"
          >
            <RequestProgress
              planApproved={stored.approved_at !== null}
              sourceApproved={Boolean(source && source.approved_at !== null)}
              coverApproved={Boolean(cover && cover.approved_at !== null)}
              inputsReceived={allReceived}
            />
            {status?.last_error && (
              <p role="alert" className="warning">
                {status.last_error}
              </p>
            )}
            {!status?.enabled && (
              <p className="warning">
                Local acquisition is not enabled. Complete the explicit development host setup
                before starting.
              </p>
            )}
            {current.state === "backup_pending" && (
              <p className="warning">
                The primary is verified. The backup is not complete. Do not treat this request as
                fully complete.
              </p>
            )}
            {current.state === "complete" && (
              <p className="success">
                The server reports this request complete. Review the publication receipts below.
              </p>
            )}
            {current.state === "awaiting_input" && (
              <p className="warning">
                Action required:{" "}
                <a href={decision.data ? `#/inbox/${decision.data.question.id}` : "#/inbox"}>
                  Open the saved decision
                </a>
                . Save an offered answer, then use its separate recovery action. Nothing starts
                automatically.
              </p>
            )}
            <details className="review-section edition-details" open={stored.approved_at === null}>
              <summary>
                Edition ·{" "}
                {stored.approved_at === null
                  ? "Review and approve the frozen edition"
                  : "Inspect saved edition and destinations"}
              </summary>
              <p className="identifier">MusicBrainz release · {stored.plan.edition.release_id}</p>
              <p>
                {stored.plan.format.toUpperCase()} · Settings version {stored.settings_version} ·{" "}
                {stored.plan.edition.tracks.length} tracks
              </p>
              <dl className="properties">
                <dt>Album artist</dt>
                <dd>{stored.plan.edition.album_artist}</dd>
                <dt>Album</dt>
                <dd>{stored.plan.edition.album}</dd>
              </dl>
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>Track</th>
                      <th>Frozen tag text</th>
                      <th>Server output preview</th>
                    </tr>
                  </thead>
                  <tbody>
                    {stored.plan.edition.tracks.map((track, index) => (
                      <tr key={index}>
                        <td>
                          {index + 1}
                          {track.disc !== null && (
                            <small>
                              Disc {track.disc} · {track.disc_track}
                            </small>
                          )}
                        </td>
                        <td>
                          <strong>{track.title}</strong>
                          <small>{track.artist}</small>
                        </td>
                        <td>
                          {stored.plan.outputs[index] ? (
                            <OutputPreview preview={stored.plan.outputs[index]} />
                          ) : (
                            <span className="warning">Server output preview is missing.</span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <h3>Approved destinations</h3>
              {stored.plan.destinations.map((destination) => (
                <div className="list-row" key={destination.location_id}>
                  <div>
                    <strong>
                      {destination.kind} · {destination.root}
                    </strong>
                    <p>
                      Coverage: {destination.coverage} · Observation:{" "}
                      {destination.observed_at === null
                        ? "incomplete / not observed"
                        : new Date(destination.observed_at * 1000).toLocaleString()}
                    </p>
                  </div>
                </div>
              ))}
              {stored.plan.accepts_incomplete_coverage && (
                <p className="warning">
                  This frozen plan accepts incomplete index coverage. Unknown coverage is not proof
                  that files are absent.
                </p>
              )}
              {stored.approved_at === null && (
                <>
                  <p>
                    Approve this exact edition, destination folders, filenames and tag text. This
                    saves the frozen plan approval only. Source, cover and download approval remain
                    separate.
                  </p>
                  <Button
                    disabled={
                      command.busy ||
                      stale ||
                      readsBlocked ||
                      stored.plan.outputs.length !== stored.plan.edition.tracks.length
                    }
                    onClick={() =>
                      void command.run(async () => {
                        await request(`${base}/approve`, { method: "POST" });
                        plan.refresh();
                      })
                    }
                  >
                    Approve frozen plan {stored.version}
                  </Button>
                </>
              )}
            </details>
            {editable ? (
              <>
                <details className="request-editor" open={!source || source.approved_at === null}>
                  <summary>
                    Source ·{" "}
                    {source?.approved_at !== null && source
                      ? "Change or inspect acknowledged source"
                      : "Choose a source"}
                  </summary>
                  <SourceReview
                    base={base}
                    releaseId={stored.plan.edition.release_id}
                    format={stored.plan.format}
                    candidates={sources.data ?? []}
                    selected={source}
                    onSelect={selectSource}
                    refresh={sources.refresh}
                    localEnabled={Boolean(status?.enabled && !status.busy)}
                  />
                  <Pager
                    busy={sources.loading}
                    previous={
                      sourcePages.length > 1
                        ? () => {
                            setSourceId(undefined);
                            setSourcePages(sourcePages.slice(0, -1));
                          }
                        : undefined
                    }
                    next={
                      sources.data?.length === 50
                        ? () => {
                            setSourceId(undefined);
                            setSourcePages([
                              ...sourcePages,
                              sources.data?.at(-1)?.id ?? sourceAfter,
                            ]);
                          }
                        : undefined
                    }
                  />
                </details>
                <details className="request-editor" open={!cover || cover.approved_at === null}>
                  <summary>
                    Cover ·{" "}
                    {cover?.approved_at !== null && cover
                      ? "Change or inspect approved cover"
                      : "Choose a cover"}
                  </summary>
                  <ArtworkReview
                    key={base}
                    base={base}
                    candidates={artwork.data ?? []}
                    selected={cover}
                    onSelect={selectCover}
                    refresh={artwork.refresh}
                  />
                  <Pager
                    busy={artwork.loading}
                    previous={
                      artworkPages.length > 1
                        ? () => {
                            setCoverId(undefined);
                            setArtworkPages(artworkPages.slice(0, -1));
                          }
                        : undefined
                    }
                    next={
                      artwork.data?.length === 50
                        ? () => {
                            setCoverId(undefined);
                            setArtworkPages([
                              ...artworkPages,
                              artwork.data?.at(-1)?.id ?? artworkAfter,
                            ]);
                          }
                        : undefined
                    }
                  />
                </details>
              </>
            ) : stored.approved_at === null ? (
              <p className="muted">
                Approve the frozen plan before selecting the source and cover.
              </p>
            ) : (
              <p className="muted">
                Source and cover changes are locked while this request is active or no longer
                queued. Saved receipts remain below.
              </p>
            )}
            {coverRecovery && (
              <>
                <p className="warning">
                  This request is safely paused for a cover decision. Prepare and approve the exact
                  JPEG below, then return to the saved decision to answer and restore its safe
                  state.
                </p>
                <ArtworkReview
                  key={`${base}:recovery`}
                  base={base}
                  candidates={artwork.data ?? []}
                  selected={cover}
                  onSelect={selectCover}
                  refresh={artwork.refresh}
                />
                <Pager
                  busy={artwork.loading}
                  previous={
                    artworkPages.length > 1
                      ? () => {
                          setCoverId(undefined);
                          setArtworkPages(artworkPages.slice(0, -1));
                        }
                      : undefined
                  }
                  next={
                    artwork.data?.length === 50
                      ? () => {
                          setCoverId(undefined);
                          setArtworkPages([
                            ...artworkPages,
                            artwork.data?.at(-1)?.id ?? artworkAfter,
                          ]);
                        }
                      : undefined
                  }
                />
              </>
            )}
            {!["publishing", "backup_pending", "complete", "cancelled"].includes(current.state) && (
              <section className="review-section">
                <h2>Start the approved download</h2>
                <p className="warning">
                  New folders only. This action connects to the configured source, downloads media,
                  writes tags in private staging, then publishes to the approved local roots.
                  Existing files are never overwritten. A failed copy stays for recovery.
                </p>
                <p>
                  Clicking the start control approves new-only publication to the frozen
                  destinations. It does not replace the separate source and exact-cover approvals.
                </p>
                {notice && <p role="status">{notice}</p>}
                <div className="actions">
                  <Button
                    disabled={!canRun}
                    onClick={() =>
                      void command.run(async () => {
                        const body: StartLocalDownload = {
                          source_candidate: allReceived ? null : (source?.id ?? null),
                          acknowledge_new_only_publication: true,
                        };
                        await request(`${base}/run`, { body });
                        setSubmitted(true);
                        setNotice(
                          "The server accepted the local run. The saved job state will show the actual result.",
                        );
                        setPoll(true);
                        refresh();
                      })
                    }
                  >
                    {command.busy
                      ? "Sending command…"
                      : allReceived
                        ? "Continue approved local work"
                        : "Start Download"}
                  </Button>
                  <Button
                    variant="secondary"
                    disabled={
                      command.busy ||
                      readsBlocked ||
                      stale ||
                      !["queued", "searching", "downloading", "validating", "normalizing"].includes(
                        current.state,
                      )
                    }
                    onClick={() =>
                      void command.run(async () => {
                        await request(`${base}/controls`, { body: { action: "pause" } });
                        setNotice(
                          "Pause requested. The worker must reach a safe boundary. Open Decisions when the saved pause question appears.",
                        );
                        refresh();
                      })
                    }
                  >
                    Pause at a safe boundary
                  </Button>
                  <CancelJob
                    key={`${id}:${version}:${current.state}`}
                    job={current}
                    disabled={command.busy || readsBlocked || stale}
                    onChanged={refresh}
                  />
                </div>
                {!canRun && current.state !== "complete" && (
                  <p className="muted">
                    Start requires a current approved plan, an acknowledged complete source, an
                    approved exact cover, local acquisition enabled and an idle worker. Clicking
                    Start Download supplies the explicit new-only acknowledgement. Uncertain
                    submissions are never retried here.
                  </p>
                )}
              </section>
            )}
          </Card>
          <section className="review-section">
            <h2>Transfer receipts</h2>
            {receipts.data?.transfers.length ? (
              <div className="table-scroll">
                <table>
                  <thead>
                    <tr>
                      <th>Track / source</th>
                      <th>Saved state</th>
                      <th>Received</th>
                    </tr>
                  </thead>
                  <tbody>
                    {receipts.data.transfers.map((file) => (
                      <tr key={file.id}>
                        <td>
                          {file.track_number} · {file.peer}
                          <small>{file.remote_path}</small>
                        </td>
                        <td>{file.state}</td>
                        <td>
                          {file.received_bytes.toLocaleString()} /{" "}
                          {file.expected_bytes.toLocaleString()} bytes
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            ) : (
              <Empty>No transfer receipts yet. A plan or acknowledgement is not a download.</Empty>
            )}
            {(receipts.data?.transfers.length ?? 0) >= 100 && (
              <p className="muted">Showing the first 100 transfer receipts.</p>
            )}
            <h2>Publication receipts</h2>
            <PublicationRecovery
              key={`${base}:${current.state}:${recoveryRevision}`}
              job={current}
              version={version}
              destinations={receipts.data?.publication ?? []}
              status={status}
              blocked={readsBlocked || stale || stored.approved_at === null}
              command={command}
              onChanged={() => {
                setPoll(true);
                refresh();
              }}
            />
            {receipts.data?.publication.length ? (
              receipts.data.publication.map((destination) => (
                <div className="list-row" key={destination.id}>
                  <div>
                    <strong>
                      {destination.kind} · {destination.root}
                    </strong>
                    <p>{destination.relative_folder}</p>
                    <p>
                      {destination.verified_at === null
                        ? "Not yet verified"
                        : `Verified ${new Date(destination.verified_at * 1000).toLocaleString()}`}
                    </p>
                    {destination.last_error && <p className="warning">{destination.last_error}</p>}
                    <small className="identifier">Manifest · {destination.manifest_sha256}</small>
                  </div>
                </div>
              ))
            ) : (
              <Empty>
                No publication receipts yet. The server has not reported a verified destination.
              </Empty>
            )}
          </section>
        </fieldset>
      )}
      <RequestHistory key={base} base={base} revision={historyRevision} active={poll} />
    </>
  );
}
