import { useEffect, useState } from "react";
import type { JobSummary } from "../src/generated/JobSummary";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import type { TransferFile } from "../src/generated/TransferFile";
import type { StartLocalDownload } from "../src/generated/StartLocalDownload";
import { OutputPreview } from "../src/OutputPreview";
import { request, type LocalStatus } from "./api";
import { useCommand, useResource } from "./hooks";
import { activeStates, canStart, stateLabel } from "./state";
import { Alert, Badge, Button, Card, Check, Empty, Pager } from "./components/ui";
import { SourceReview } from "./SourceReview";
import { ArtworkReview } from "./ArtworkReview";

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
  const [approvedOutput, setApprovedOutput] = useState(false);
  const [newOnly, setNewOnly] = useState(false);
  const [submitted, setSubmitted] = useState(false);
  const [notice, setNotice] = useState("");
  const command = useCommand();
  const stored = plan.data;
  const current = job.data;
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
    job.refresh();
    plan.refresh();
    sources.refresh();
    artwork.refresh();
    receipts.refresh();
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
  const editable = Boolean(
    stored?.approved_at !== null && stored && current?.state === "queued" && !submitted && !stale,
  );
  const canRun = Boolean(
    current &&
    stored &&
    canStart(current, stored, source, cover, status, newOnly, allReceived) &&
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
          {current && <Badge>{stateLabel(current.state)}</Badge>}
        </div>
        <Button variant="secondary" disabled={command.busy} onClick={refresh}>
          Refresh request
        </Button>
      </header>
      <Alert error={job.error || plan.error || sources.error || artwork.error || receipts.error} />
      <Alert error={command.error} />
      {(job.loading || plan.loading) && !stored && <p role="status">Loading saved request…</p>}
      {stale && (
        <p className="warning">
          This plan is no longer current.{" "}
          <a href={`#/request/${id}/${current?.plan_version}`}>Open the current plan.</a>
        </p>
      )}
      {stored && current && (
        <>
          <Card title="What you need to do" className="request-summary">
            <ul className="checklist">
              <li>
                Edition{" "}
                <Badge>
                  {stored.approved_at !== null ? "Plan approved" : "Review output names and tags"}
                </Badge>
              </li>
              <li>
                Source{" "}
                <Badge>
                  {source?.approved_at !== null && source
                    ? "Acknowledgement saved"
                    : "Choose and acknowledge exact source"}
                </Badge>
              </li>
              <li>
                Cover{" "}
                <Badge>
                  {cover?.approved_at !== null && cover
                    ? "Exact JPEG approved"
                    : "Choose and approve exact preview"}
                </Badge>
              </li>
            </ul>
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
                Action required: inspect this request's saved question with the operator CLI. Answer
                it and run the matching recovery command there, then refresh this page. This page
                never answers questions or resumes work automatically.
              </p>
            )}
          </Card>
          <section className="review-section">
            <h2>Review the frozen edition and outputs</h2>
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
                <Check
                  checked={approvedOutput}
                  onChange={setApprovedOutput}
                  disabled={command.busy || stale}
                >
                  I reviewed this exact edition, destination folders, filenames and tag text.
                </Check>
                <Button
                  disabled={
                    command.busy ||
                    stale ||
                    !approvedOutput ||
                    stored.plan.outputs.length !== stored.plan.edition.tracks.length
                  }
                  onClick={() =>
                    void command.run(async () => {
                      await request(`${base}/approve`, { method: "POST" });
                      setApprovedOutput(false);
                      plan.refresh();
                    })
                  }
                >
                  Approve frozen plan {stored.version}
                </Button>
              </>
            )}
          </section>
          {editable ? (
            <>
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
                        setSourcePages([...sourcePages, sources.data?.at(-1)?.id ?? sourceAfter]);
                      }
                    : undefined
                }
              />
              <ArtworkReview
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
          ) : stored.approved_at === null ? (
            <p className="muted">Approve the frozen plan before selecting the source and cover.</p>
          ) : (
            <p className="muted">
              Source and cover changes are locked while this request is active or no longer queued.
              Saved receipts remain below.
            </p>
          )}
          <section className="review-section">
            <h2>Start the approved download</h2>
            <p className="warning">
              New folders only. This action connects to the configured source, downloads media,
              writes tags in private staging, then publishes to the approved local roots. Existing
              files are never overwritten. A failed copy stays for recovery.
            </p>
            <Check checked={newOnly} onChange={setNewOnly} disabled={command.busy || submitted}>
              I approve new-only publication to the destinations shown above.
            </Check>
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
                    setNewOnly(false);
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
                variant="destructive"
                disabled={
                  command.busy ||
                  stale ||
                  ![
                    "queued",
                    "searching",
                    "downloading",
                    "validating",
                    "normalizing",
                    "awaiting_input",
                  ].includes(current.state)
                }
                onClick={() =>
                  void command.run(async () => {
                    await request(`${base}/controls`, { body: { action: "cancel" } });
                    setNotice(
                      "Safe stop requested. In-flight work and receipts are preserved until a safe boundary.",
                    );
                    refresh();
                  })
                }
              >
                Cancel request safely
              </Button>
            </div>
            {!canRun && current.state !== "complete" && (
              <p className="muted">
                Start requires a current approved plan, an acknowledged complete source, an approved
                exact cover, local acquisition enabled, an idle worker and the new-only
                acknowledgement. Uncertain submissions are never retried here.
              </p>
            )}
          </section>
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
        </>
      )}
    </>
  );
}
