import { useState } from "react";
import type { TagPolicy } from "../src/generated/TagPolicy";
import type { TagRule } from "../src/generated/TagRule";
import type { TagReview } from "../src/generated/TagReview";
import { request } from "./api";
import { useCommand, useMetadataResource } from "./hooks";
import { Alert, Button, Card, Pager } from "./components/ui";

const fields = [
  ["genre", "Genre"],
  ["composer", "Composer"],
  ["comment", "Comment"],
  ["copyright", "Copyright"],
] as const;
const reviewPageSize = 50;

export function defaultTagPolicy(): TagPolicy {
  return {
    genre: { action: "remove", mode: "automatic" },
    composer: { action: "remove", mode: "automatic" },
    comment: { action: "remove", mode: "automatic" },
    copyright: { action: "remove", mode: "automatic" },
  };
}

export function needsTagReview(policy: TagPolicy | null | undefined): boolean {
  return Boolean(
    policy &&
    fields.some(([key]) => {
      const rule = policy[key];
      return rule.action === "retain" || rule.mode === "review";
    }),
  );
}

export function TagPolicyFields({
  policy,
  disabled,
  onChange,
}: {
  policy: TagPolicy;
  disabled: boolean;
  onChange: (policy: TagPolicy) => void;
}) {
  function change(key: (typeof fields)[number][0], rule: TagRule) {
    onChange({ ...policy, [key]: rule });
  }
  return (
    <fieldset disabled={disabled}>
      <legend>Optional tag rules</legend>
      <p>
        These shared rules apply to new plans. Release tags and approved artwork stay locked.
        Unknown tags are removed. Retain always needs exact review after download.
      </p>
      {fields.map(([key, label]) => {
        const rule = policy[key];
        return (
          <div key={key}>
            <label>
              {label} action
              <select
                value={rule.action}
                onChange={(event) => {
                  const action = event.target.value;
                  if (action === "retain") change(key, { action });
                  else if (action === "update")
                    change(key, { action, value: "", mode: "automatic" });
                  else if (action === "remove") change(key, { action, mode: "automatic" });
                }}
              >
                <option value="remove">Remove</option>
                <option value="retain">Retain exact source text</option>
                <option value="update">Update with fixed text</option>
              </select>
            </label>
            {rule.action === "update" && (
              <label>
                {label} value
                <input
                  required
                  value={rule.value}
                  onChange={(event) => change(key, { ...rule, value: event.target.value })}
                />
                <small>
                  At most 4,096 UTF-8 bytes. No control characters. Copyright must start with a
                  four-digit year and a space.
                </small>
              </label>
            )}
            {rule.action === "retain" ? (
              <p>{label} mode: Review required</p>
            ) : (
              <label>
                {label} mode
                <select
                  value={rule.mode}
                  onChange={(event) => {
                    const mode = event.target.value;
                    if (mode === "automatic" || mode === "review") change(key, { ...rule, mode });
                  }}
                >
                  <option value="automatic">Automatic under the approved plan</option>
                  <option value="review">Review after download</option>
                </select>
              </label>
            )}
          </div>
        );
      })}
    </fieldset>
  );
}

export function FrozenTagPolicy({ policy }: { policy: TagPolicy | null | undefined }) {
  return (
    <div>
      <h3>Frozen optional tag rules</h3>
      {policy ? (
        <ul>
          {fields.map(([key, label]) => {
            const rule = policy[key];
            return (
              <li key={key}>
                {label}: {rule.action}
                {rule.action === "update" && (
                  <>
                    {' to "'}
                    <span style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
                      {rule.value}
                    </span>
                    {'"'}
                  </>
                )}
                {" · "}
                {rule.action === "retain" ? "Review required" : rule.mode}
              </li>
            );
          })}
        </ul>
      ) : (
        <p>Legacy plan: write release tags and approved artwork. Remove other source metadata.</p>
      )}
      <p>
        {needsTagReview(policy)
          ? "Source values are unknown until download finishes. Exact tag approval is required before normalization."
          : "No extra tag approval is required. Automatic rules do not approve downloads, artwork, or replacement."}
      </p>
    </div>
  );
}

export function TagReviewPanel({
  job,
  version,
  disabled = false,
  onChanged,
}: {
  job: number;
  version: number;
  disabled?: boolean;
  onChanged: () => void;
}) {
  const [cursors, setCursors] = useState([0]);
  const after = cursors[cursors.length - 1] ?? 0;
  const base = `/api/jobs/${job}/plans/${version}/tags`;
  const review = useMetadataResource(
    (signal) => request<TagReview | null>(`${base}?after=${after}&limit=50`, { signal }),
    `${base}:${after}`,
  );
  const command = useCommand();
  const [submitted, setSubmitted] = useState<"approve" | "resume" | null>(null);
  const saved = review.data;
  const allTracksLoaded = Boolean(saved && saved.tracks.length < reviewPageSize);
  const blocked =
    disabled ||
    review.loading ||
    Boolean(review.error) ||
    command.busy ||
    (submitted === "approve" && !saved?.approved) ||
    (submitted === "resume" && !saved?.resumed);
  async function act(action: "approve" | "resume") {
    if (blocked || !saved) return;
    await request(`${base}/${action}`, { body: { sha256: saved.sha256 } });
    setSubmitted(action);
    onChanged();
    review.refresh();
  }
  return (
    <Card title="Exact downloaded tag review">
      <Button
        variant="secondary"
        disabled={command.busy || review.loading}
        onClick={() => {
          setSubmitted(null);
          review.refresh();
        }}
      >
        Reload tag review
      </Button>
      <Alert error={review.error || command.error} />
      {review.loading && <p role="status">Reading tag review...</p>}
      {saved === null && <p>Exact source tags will be available after all inputs are received.</p>}
      {saved && (
        <>
          <FrozenTagPolicy policy={saved.policy} />
          <p>
            Approval covers every track in this exact album manifest. Check all pages before
            approval. Required release tags and the approved cover remain unchanged.
          </p>
          {saved.tracks.map((track) => (
            <div key={track.track_number}>
              <h4>Track {track.track_number}</h4>
              <dl>
                {fields.map(([key, label]) => (
                  <div key={key}>
                    <dt>{label}</dt>
                    <dd>
                      Source:{" "}
                      <span style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
                        {saved.policy[key].action !== "retain" &&
                        saved.policy[key].mode === "automatic"
                          ? "Not inspected for automatic rule"
                          : (track.source[key] ?? "Absent")}
                      </span>
                      {" → "}
                      Output:{" "}
                      <span style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>
                        {track.output[key] ?? "Absent"}
                      </span>
                      {key === "comment" &&
                        (track.source.comment_language || track.output.comment_language) && (
                          <p>
                            MP3 language: {track.source.comment_language ?? "Absent"} to{" "}
                            {track.output.comment_language ?? "Absent"}
                          </p>
                        )}
                    </dd>
                  </div>
                ))}
              </dl>
            </div>
          ))}
          {!allTracksLoaded && (
            <p role="status">Review this page and load the next page before approving the album.</p>
          )}
          {allTracksLoaded && <p role="status">All tracks are available for this album review.</p>}
          <Pager
            busy={blocked}
            {...(cursors.length > 1
              ? { previous: () => setCursors((current) => current.slice(0, -1)) }
              : {})}
            {...(saved.tracks.length === reviewPageSize
              ? {
                  next: () =>
                    setCursors((current) => [
                      ...current,
                      saved.tracks[saved.tracks.length - 1]?.track_number ?? after,
                    ]),
                }
              : {})}
          />
          {!saved.approved ? (
            <Button
              disabled={blocked || !allTracksLoaded}
              onClick={() => void command.run(() => act("approve"))}
            >
              Approve exact tags for this album
            </Button>
          ) : !saved.resumed ? (
            <Button
              disabled={blocked || !allTracksLoaded}
              onClick={() => void command.run(() => act("resume"))}
            >
              Apply saved tag approval
            </Button>
          ) : (
            <p>Tag approval applied. Use the separate request command to continue local work.</p>
          )}
          <p>Neither approval nor applying it starts a worker or a download.</p>
        </>
      )}
    </Card>
  );
}
