import type { TransferSummary } from "../src/generated/TransferSummary";

export function allInputsReceived(
  summary: TransferSummary | undefined,
  job: number,
  version: number,
  tracks: number,
) {
  return Boolean(
    summary &&
    summary.job_id === job &&
    summary.plan_version === version &&
    summary.expected_tracks === tracks &&
    tracks > 0 &&
    summary.total === tracks &&
    summary.received === tracks &&
    summary.planned === 0 &&
    summary.submitting === 0 &&
    summary.active === 0 &&
    summary.all_received,
  );
}

export function TransferProgress({ summary }: { summary: TransferSummary }) {
  return (
    <div aria-label="Whole-album saved transfer counts">
      <p>
        {summary.received} of {summary.expected_tracks} tracks received · {summary.total} saved
        transfer intents
      </p>
      <p className="muted">
        {summary.planned} planned · {summary.submitting} submitting · {summary.active} active
      </p>
      {summary.submitting > 0 && (
        <p className="warning">
          Tracks with uncertain submission: {summary.submitting}. A receipt page cannot authorize a
          retry. Inspect saved state before further work.
        </p>
      )}
      <p className="muted">
        Counts cover this whole plan, not only the receipt page below. Saved received receipts do
        not prove current staging contents or completed publication. The worker rechecks them.
      </p>
    </div>
  );
}
