import { useEffect, useState } from "react";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import type { AlbumReplacementPreview } from "../src/generated/AlbumReplacementPreview";
import type { AlbumReplacementReview as SavedReplacementReview } from "../src/generated/AlbumReplacementReview";
import type { TransferFile } from "../src/generated/TransferFile";
import type { TransferSummary } from "../src/generated/TransferSummary";
import type { StartLocalDownload } from "../src/generated/StartLocalDownload";
import type { QuestionDetails } from "../src/generated/QuestionDetails";
import type { AudioProfileDescriptor } from "../src/generated/AudioProfileDescriptor";
import { audioProfileLabel } from "./audio-profiles";
import { OutputPreview } from "../src/OutputPreview";
import { request, type LocalStatus } from "./api";
import { useCommand, useResource } from "./hooks";
import { activeStates, canStart } from "./state";
import { Alert, Badge, Button, Card, Empty, Pager } from "./components/ui";
import { SourceReview } from "./SourceReview";
import { WorkerRequests } from "./WorkerRequests";
import { ArtworkReview } from "./ArtworkReview";
import { CancelJob, JobStatus } from "./components/Media";
import { RequestHistory } from "./RequestHistory";
import { AlbumReplacementRestore, PublicationRecovery } from "./PublicationRecovery";
import { AlbumReplacementReview } from "./AlbumReplacementReview";
import { TransferProgress, allInputsReceived } from "./TransferProgress";

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
  audioProfiles = [],
}: {
  id: number;
  version: number;
  status: LocalStatus | undefined;
  onChanged: () => void;
  audioProfiles?: AudioProfileDescriptor[] | undefined;
}) {
  const base = `/api/jobs/${id}/plans/${version}`;
  const [poll, setPoll] = useState(false);
  const [sourcePages, setSourcePages] = useState([0]);
  const [artworkPages, setArtworkPages] = useState([0]);
  const [transferPages, setTransferPages] = useState([0]);
  const transferAfter = transferPages.at(-1) ?? 0;
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
    setPoll(
      Boolean(
        status?.busy ||
        status?.worker?.unfinished ||
        (job.data && activeStates.has(job.data.state)),
      ),
    );
  }, [status?.busy, status?.worker?.unfinished, job.data]);
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
      const [publication, transfers, summary] = await Promise.all([
        request<PublicationDestination[]>(`${base}/publication`, { signal }),
        request<TransferFile[]>(`${base}/transfers?after=${transferAfter}&limit=50`, { signal }),
        request<TransferSummary>(`${base}/transfers/summary`, { signal }),
      ]);
      return { publication, transfers, summary };
    },
    `${base}:${job.data?.state ?? ""}:${transferAfter}`,
    poll ? 2000 : 0,
  );
  const replacement = useResource(
    (signal) =>
      job.data?.state === "queued" && plan.data?.approved_at !== null
        ? request<AlbumReplacementPreview[]>(`${base}/publication/replacement-review`, { signal })
        : Promise.resolve([]),
    `${base}:${job.data?.state ?? ""}:${plan.data?.approved_at ?? ""}`,
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
  const allReceived =
    !receipts.loading &&
    !receipts.error &&
    allInputsReceived(receipts.data?.summary, id, version, stored?.plan.edition.tracks.length ?? 0);
  function refresh() {
    setHistoryRevision((value) => value + 1);
    job.refresh();
    plan.refresh();
    sources.refresh();
    artwork.refresh();
    receipts.refresh();
    replacement.refresh();
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
  const replacementBlocked = Boolean(
    current?.state === "queued" &&
    stored?.approved_at !== null &&
    (!replacement.data ||
      replacement.loading ||
      replacement.error ||
      replacement.data.some(
        (preview) =>
          (!preview.destination_available && preview.kind === "main") ||
          (preview.destination_available &&
            preview.album_identity !== null &&
            (!preview.decision_saved || preview.saved_action === null)),
      )),
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
    !replacementBlocked &&
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
      <p>
        <a href={`#/recovery?job=${id}`}>Open request Recovery</a>
        {" · "}Inspect retained versions and scratch cleanup separately from album completion.
      </p>
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
                Acquisition is not enabled. Configure the production worker or the explicit
                development runtime before starting.
              </p>
            )}
            {status?.worker && <WorkerRequests key={id} status={status.worker} job={id} />}
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
                {stored.plan.audio
                  ? audioProfileLabel(stored.plan.audio.profile, audioProfiles)
                  : stored.plan.format}{" "}
                · Settings version {stored.settings_version} · {stored.plan.edition.tracks.length}{" "}
                tracks
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
                    audioProfile={stored.plan.audio?.profile ?? null}
                    audioProfiles={audioProfiles}
                    candidates={sources.data ?? []}
                    selected={source}
                    onSelect={selectSource}
                    refresh={sources.refresh}
                    localEnabled={Boolean(status?.enabled && !status.busy && !status.worker)}
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
                <h2>
                  {allReceived ? "Continue approved local work" : "Start the approved download"}
                </h2>
                <p className="warning">
                  {allReceived
                    ? "The worker rechecks retained inputs, then continues approved local processing and publishes according to the saved decision for each exact destination. It does not download again."
                    : "This action connects to the configured source, downloads media, writes tags in private staging, then publishes to the approved local roots according to the saved decision for each exact destination."}{" "}
                  The server never overwrites an unreviewed folder. A failed copy stays for
                  recovery.
                </p>
                <p>
                  Clicking start confirms publication to the frozen destinations. Existing folders
                  need a separate exact decision below before the worker can act on them. This does
                  not replace the source and exact-cover approvals.
                </p>
                {current.state === "queued" && stored.approved_at !== null && (
                  <>
                    {replacement.error && (
                      <p className="warning">
                        The current destination review failed. The saved plan cannot be started
                        until every destination has a current review.
                      </p>
                    )}
                    {replacement.data && (
                      <AlbumReplacementReview
                        previews={replacement.data}
                        blocked={replacement.loading || Boolean(replacement.error) || readsBlocked}
                        command={command}
                        onApproved={(_saved: SavedReplacementReview) => refresh()}
                      />
                    )}
                  </>
                )}
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
                          status?.worker
                            ? "The worker request is saved. The saved job state will show the actual result."
                            : "The server accepted the local run. The saved job state will show the actual result.",
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
                    Start Download supplies the explicit publication acknowledgement. Uncertain
                    submissions are never retried here.
                  </p>
                )}
              </section>
            )}
          </Card>
          <section className="review-section">
            <h2>Transfer receipts</h2>
            {receipts.error && receipts.data ? (
              <p className="warning">
                The receipt read failed. These retained counts and rows are not a current response.
                Use Refresh request to read them again. Commands and receipt pages stay disabled.
              </p>
            ) : null}
            {receipts.loading && <p role="status">Reading saved receipts…</p>}
            {receipts.data?.summary && <TransferProgress summary={receipts.data.summary} />}
            {receipts.data?.transfers.length ? (
              <div
                className="table-scroll"
                role="region"
                aria-label="Transfer receipt rows"
                tabIndex={0}
              >
                <table className="transfer-receipts">
                  <thead>
                    <tr>
                      <th scope="col">Track / source</th>
                      <th scope="col">Saved state</th>
                      <th scope="col">Received</th>
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
            ) : !receipts.loading && !receipts.error && receipts.data ? (
              <Empty>
                {transferPages.length > 1
                  ? "No transfer receipts on this page."
                  : "No transfer receipts yet. A plan or acknowledgement is not a download."}
              </Empty>
            ) : null}
            {receipts.data && (
              <div aria-label="Transfer receipt pages">
                <p className="muted">
                  Receipt page {transferPages.length} · Up to 50 rows per page
                </p>
                <Pager
                  busy={receipts.loading || Boolean(receipts.error) || command.busy}
                  previous={
                    transferPages.length > 1
                      ? () => setTransferPages(transferPages.slice(0, -1))
                      : undefined
                  }
                  next={
                    receipts.data.transfers.length === 50
                      ? () =>
                          setTransferPages([
                            ...transferPages,
                            receipts.data?.transfers.at(-1)?.id ?? transferAfter,
                          ])
                      : undefined
                  }
                />
              </div>
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
            <AlbumReplacementRestore
              key={`${base}:${current.state}:${recoveryRevision}:${historyRevision}`}
              job={current}
              version={version}
              destinations={receipts.data?.publication ?? []}
              blocked={readsBlocked || stale || stored.approved_at === null}
              command={command}
              onChanged={() => {
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
                    <small className="identifier">
                      Historical publication manifest · {destination.manifest_sha256}
                    </small>
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
