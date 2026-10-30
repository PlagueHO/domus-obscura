import { useState } from "react";
import type { SourceCandidate } from "../src/generated/SourceCandidate";
import type { SourceFolderReport } from "../src/generated/SourceFolderReport";
import type { ApproveSourceCandidate } from "../src/generated/ApproveSourceCandidate";
import type { AudioFormat } from "../src/generated/AudioFormat";
import type { AudioProfile } from "../src/generated/AudioProfile";
import type { AudioProfileDescriptor } from "../src/generated/AudioProfileDescriptor";
import type { SourceSearchRequest } from "../src/generated/SourceSearchRequest";
import type { SourceFolderRequest } from "../src/generated/SourceFolderRequest";
import { request, type SearchReport } from "./api";
import { useCommand } from "./hooks";
import { Alert, Badge, Button, Check, Empty } from "./components/ui";
import { audioProfileLabel } from "./audio-profiles";

export function SourceReview({
  base,
  releaseId,
  format,
  audioProfile = null,
  audioProfiles = [],
  candidates,
  selected,
  onSelect,
  refresh,
  localEnabled,
}: {
  base: string;
  releaseId: string;
  format: AudioFormat;
  audioProfile?: AudioProfile | null;
  audioProfiles?: AudioProfileDescriptor[];
  candidates: SourceCandidate[];
  selected: SourceCandidate | undefined;
  onSelect: (id: number) => void;
  refresh: () => void;
  localEnabled: boolean;
}) {
  const [peer, setPeer] = useState("");
  const [folder, setFolder] = useState("");
  const [report, setReport] = useState<SearchReport>();
  const [acknowledged, setAcknowledged] = useState(false);
  const [offset, setOffset] = useState(0);
  const command = useCommand();
  const groups = report?.groups ?? [];
  const fileCount = groups.reduce((count, group) => count + group.files.length, 0);

  async function assess() {
    const body: SourceFolderRequest = {
      release_id: releaseId,
      peer,
      remote_folder: folder,
      format,
      audio_profile: audioProfile,
    };
    const result = await request<SourceFolderReport>("/api/local/source-folder", {
      body,
    });
    await importListing(result.listing);
  }
  async function importListing(listing: unknown) {
    const candidate = await request<SourceCandidate>(`${base}/source-candidates`, {
      body: listing,
    });
    onSelect(candidate.id);
    setAcknowledged(false);
    refresh();
  }
  async function importFile(file: File | undefined) {
    if (!file) return;
    if (file.size > 1_048_576) throw new Error("The listing must be at most 1 MiB.");
    const listing: unknown = JSON.parse(await file.text());
    if (typeof listing !== "object" || listing === null || Array.isArray(listing)) {
      throw new Error("Select a SourceFolder JSON object, not a path or report.");
    }
    await importListing(listing);
  }
  async function approve() {
    if (!selected || !acknowledged) return;
    const body: ApproveSourceCandidate = {
      plan_version: selected.plan_version,
      review_sha256: selected.review_sha256,
      acknowledge_exact_edition_and_permitted_download: true,
    };
    await request(`/api/source-candidates/${selected.id}/approve`, { body });
    setAcknowledged(false);
    refresh();
  }
  return (
    <section className="review-section" aria-labelledby="source-heading">
      <h2 id="source-heading">Choose the source</h2>
      <p>
        Search and folder assessment read metadata only. They do not prove edition, encoding or
        permission. No audio is downloaded here.
      </p>
      <p>
        Frozen plan profile:{" "}
        {audioProfile ? audioProfileLabel(audioProfile, audioProfiles) : format}. Other accepted
        profiles require a new plan and separate approval.
      </p>
      <Button
        variant="secondary"
        disabled={command.busy || !localEnabled}
        onClick={() =>
          void command.run(async () => {
            const body: SourceSearchRequest = { release_id: releaseId };
            setReport(await request<SearchReport>("/api/local/source-search", { body }));
            setOffset(0);
          })
        }
      >
        {command.busy ? "Checking source metadata…" : "Search Soulseek for this release"}
      </Button>
      {!localEnabled && (
        <p className="warning">
          Live source search is not enabled in this host's local setup. You can import an operator
          listing below.
        </p>
      )}
      {report && (
        <>
          <p>
            {report.query} · {report.peers.length} peers · {groups.length} matching folders ·{" "}
            {fileCount} files
            {report.limit_reached ? " · Search limit reached" : ""}
          </p>
          {report.issues.map((issue, index) => (
            <p className="warning" key={`${issue.peer}:${index}`}>
              {issue.peer}: {issue.error}
            </p>
          ))}
          {groups.length === 0 && (
            <Empty>
              No accepted-profile files were returned. You can enter a peer and folder below.
            </Empty>
          )}
          <SourceSearchResults
            report={report}
            groups={sourceSearchPage(groups, offset)}
            audioProfiles={audioProfiles}
            busy={command.busy}
            onFolder={(peer, folder) => {
              setPeer(peer);
              setFolder(folder);
            }}
          />
          <div className="actions">
            <Button
              variant="secondary"
              disabled={offset === 0}
              onClick={() => setOffset(Math.max(0, offset - 30))}
            >
              Previous files
            </Button>
            <Button
              variant="secondary"
              disabled={offset + 30 >= fileCount}
              onClick={() => setOffset(offset + 30)}
            >
              Next files
            </Button>
          </div>
        </>
      )}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void command.run(assess);
        }}
        className="source-form"
      >
        <label>
          Peer username
          <input
            required
            maxLength={256}
            value={peer}
            disabled={command.busy}
            onChange={(event) => setPeer(event.target.value)}
            autoComplete="off"
          />
        </label>
        <label>
          Remote folder
          <input
            required
            maxLength={4096}
            value={folder}
            disabled={command.busy}
            onChange={(event) => setFolder(event.target.value)}
            autoComplete="off"
          />
        </label>
        <Button
          type="submit"
          variant="secondary"
          disabled={command.busy || !localEnabled || !peer.trim() || !folder.trim()}
        >
          Assess and import this folder
        </Button>
      </form>
      <label className="file-label">
        Or import a bounded SourceFolder JSON listing
        <input
          type="file"
          accept=".json,application/json"
          disabled={command.busy}
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            void command.run(() => importFile(file));
          }}
        />
      </label>
      <Alert error={command.error} />
      {candidates.length > 0 && (
        <label>
          Saved source listing
          <select
            value={selected?.id ?? ""}
            disabled={command.busy}
            onChange={(event) => {
              onSelect(Number(event.target.value));
              setAcknowledged(false);
            }}
          >
            <option value="" disabled>
              Choose a saved listing
            </option>
            {candidates.map((candidate) => (
              <option value={candidate.id} key={candidate.id}>
                #{candidate.id} · {candidate.listing.peer} · {candidate.listing.folder}
                {candidate.approved_at !== null ? " · acknowledged" : ""}
              </option>
            ))}
          </select>
        </label>
      )}
      {selected && (
        <div className="source-assessment">
          <h3>
            {selected.listing.peer} · {selected.listing.folder}
          </h3>
          <Badge>
            {selected.assessment.complete_mapping
              ? "Complete track mapping"
              : "Mapping needs attention"}
          </Badge>
          <p>
            {audioProfileLabel(selected.assessment.profile, audioProfiles)} · Advertised quality{" "}
            {selected.assessment.advertised_quality_matches
              ? "matches"
              : selected.assessment.problems.some(
                    (problem) => problem.issue === "advertised_quality_mismatch",
                  )
                ? "does not match"
                : "is unknown"}{" "}
            · Audio not verified by metadata review
          </p>
          {selected.alternatives.length > 0 && (
            <section aria-label="Other accepted profiles">
              <h3>Other accepted profiles for review</h3>
              <p>
                These results use the complete folder listing. They do not change this frozen plan
                or acknowledge a source. To use another profile, create a new plan and approve it
                separately. No fallback or download starts automatically.
              </p>
              <ul>
                {selected.alternatives.map(({ preferred, assessment }) => (
                  <li key={assessment.profile}>
                    {audioProfileLabel(assessment.profile, audioProfiles)}
                    {preferred ? " · Preferred profile" : ""} ·{" "}
                    {assessment.complete_mapping
                      ? "Complete track mapping"
                      : "Mapping needs attention"}{" "}
                    · Advertised quality{" "}
                    {assessment.advertised_quality_matches
                      ? "matches"
                      : assessment.problems.some(
                            (problem) => problem.issue === "advertised_quality_mismatch",
                          )
                        ? "does not match"
                        : "is unknown"}
                    {" · Audio not verified"}
                  </li>
                ))}
              </ul>
            </section>
          )}
          {selected.assessment.can_approve && !selected.assessment.advertised_quality_matches && (
            <p>
              The peer did not supply all quality attributes. You can acknowledge this source.
              Downloaded audio must pass full quality checks before tagging or publication.
            </p>
          )}
          {selected.assessment.problems.length > 0 && (
            <ul className="warning">
              {selected.assessment.problems.map((problem, index) => (
                <li key={index}>
                  {problem.issue} · Track {problem.track_number ?? "—"} · {problem.remote_path}
                </li>
              ))}
            </ul>
          )}
          <details>
            <summary>Review all mapped tracks ({selected.assessment.tracks.length})</summary>
            <ol>
              {selected.assessment.tracks.map((track) => (
                <li key={track.track_number}>
                  Track {track.track_number}: {track.remote_path} ·{" "}
                  {track.expected_bytes.toLocaleString()} bytes · {track.rule}
                </li>
              ))}
            </ol>
          </details>
          <p className="identifier">Review digest · {selected.review_sha256}</p>
          {selected.approved_at !== null ? (
            <p className="success">
              Your source acknowledgement is saved. It is not independent edition or rights proof.
            </p>
          ) : (
            <>
              <Check
                checked={acknowledged}
                onChange={setAcknowledged}
                disabled={command.busy || !selected.assessment.can_approve}
              >
                I checked that this is the exact edition, and I have permission to download it.
              </Check>
              <Button
                disabled={command.busy || !acknowledged || !selected.assessment.can_approve}
                onClick={() => void command.run(approve)}
              >
                Acknowledge this exact source
              </Button>
            </>
          )}
        </div>
      )}
    </section>
  );
}

/** Page file metadata without losing its peer/folder grouping. */
export function sourceSearchPage(
  groups: SearchReport["groups"],
  offset: number,
): SearchReport["groups"] {
  let start = 0;
  return groups.flatMap((group) => {
    const files = group.files.slice(Math.max(0, offset - start), Math.max(0, offset + 30 - start));
    start += group.files.length;
    return files.length > 0 ? [{ ...group, files }] : [];
  });
}

export function SourceSearchResults({
  report,
  groups = report.groups,
  audioProfiles = [],
  busy,
  onFolder,
}: {
  report: SearchReport;
  groups?: SearchReport["groups"];
  audioProfiles?: AudioProfileDescriptor[];
  busy: boolean;
  onFolder: (peer: string, folder: string) => void;
}) {
  return (
    <>
      <p className="muted">
        Files are grouped by peer and folder under the saved audio policy. Metadata matches do not
        prove a complete album. Assess the full folder before source review. No profile is selected
        automatically.
      </p>
      <div className="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Peer / folder</th>
              <th>Advertised files and profiles</th>
              <th>Queue</th>
              <th>Folder</th>
            </tr>
          </thead>
          <tbody>
            {groups.map((group) => {
              const peer = report.peers.find((peer) => peer.peer === group.peer);
              return (
                <tr key={`${group.peer}:${group.folder}`}>
                  <td>
                    <strong>{group.peer}</strong>
                    <small>{group.folder}</small>
                  </td>
                  <td>
                    {group.files.map(({ file, matches }) => (
                      <div key={file.remote_path}>
                        <strong>{file.remote_path}</strong>
                        <small>
                          {file.extension} · {file.bitrate_kbps ?? "Unknown"} kbps ·{" "}
                          {file.vbr === true
                            ? "VBR advertised"
                            : file.vbr === false
                              ? "CBR advertised"
                              : "CBR/VBR unknown"}
                          {" · "}
                          {file.expected_bytes.toLocaleString()} bytes
                        </small>
                        {matches.map((match) => (
                          <small key={match.profile}>
                            {audioProfileLabel(match.profile, audioProfiles)}
                            {match.preferred ? " · Preferred profile" : ""} ·{" "}
                            {match.quality === "match"
                              ? "Metadata match"
                              : match.quality === "mismatch"
                                ? "Metadata mismatch"
                                : "Quality unknown"}
                          </small>
                        ))}
                      </div>
                    ))}
                  </td>
                  <td>
                    {peer
                      ? peer.slot_free
                        ? "Slot available"
                        : `${peer.queue_length} queued`
                      : "Queue unknown"}
                  </td>
                  <td>
                    <Button
                      variant="secondary"
                      disabled={busy}
                      onClick={() => onFolder(group.peer, group.folder)}
                    >
                      Use this folder
                    </Button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </>
  );
}
