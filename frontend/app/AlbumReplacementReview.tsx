import { useState } from "react";
import type { AlbumReplacementDecision } from "../src/generated/AlbumReplacementDecision";
import type { AlbumReplacementPreview } from "../src/generated/AlbumReplacementPreview";
import type { AlbumReplacementReview as SavedReview } from "../src/generated/AlbumReplacementReview";
import { request } from "./api";
import type { useCommand } from "./hooks";
import { Button } from "./components/ui";

export function AlbumReplacementReview({
  previews,
  blocked,
  command,
  onApproved,
}: {
  previews: AlbumReplacementPreview[];
  blocked: boolean;
  command: ReturnType<typeof useCommand>;
  onApproved: (review: SavedReview) => void;
}) {
  const [actions, setActions] = useState<Record<number, AlbumReplacementDecision["action"]>>({});
  const unavailable = previews.filter((preview) => !preview.destination_available);
  const collisions = previews.filter(
    (preview) => preview.destination_available && preview.album_identity !== null,
  );
  if (collisions.length === 0 && unavailable.length === 0) return null;
  return (
    <section className="review-section" aria-label="Album replacement review">
      <h2>Existing album folders</h2>
      {unavailable.map((preview) => (
        <p className="warning" key={preview.location_id}>
          {preview.kind} destination {preview.root} is unavailable:{" "}
          {preview.unavailable_reason ?? "the server cannot read this destination"}.{" "}
          {preview.kind === "main"
            ? "The primary publication needs this destination to be available."
            : "The primary publication can continue. This destination will remain pending until it is available."}
        </p>
      ))}
      {collisions.length > 0 && (
        <p>
          Review every existing file and the proposed complete album before you choose what to do.
          The server checks these exact files and folder identities again before it changes
          anything.
        </p>
      )}
      {collisions.map((preview) => {
        const selected = actions[preview.location_id] ?? preview.saved_action ?? undefined;
        const isApproved = preview.decision_saved && preview.saved_action !== null;
        return (
          <article className="list-row" key={preview.location_id}>
            <div>
              <strong>
                {preview.kind} · {preview.root} · {preview.relative_folder}
              </strong>
              <p>Current files</p>
              {preview.existing_files.length > 0 ? (
                <ul>
                  {preview.existing_files.map((file) => (
                    <li key={file.name}>
                      {file.name} · {file.bytes.toLocaleString()} bytes
                    </li>
                  ))}
                </ul>
              ) : (
                <p>The existing folder is empty.</p>
              )}
              <p>Proposed files</p>
              <ul>
                {preview.output_files.map((name) => (
                  <li key={name}>{name}</li>
                ))}
              </ul>
              {isApproved && <p role="status">Decision saved for this exact review.</p>}
              <div
                className="actions"
                role="group"
                aria-label={`Choose action for ${preview.kind}`}
              >
                <Button
                  variant="secondary"
                  disabled={blocked || command.busy}
                  aria-pressed={selected === "skip"}
                  onClick={() =>
                    setActions((current) => ({ ...current, [preview.location_id]: "skip" }))
                  }
                >
                  Skip this album
                </Button>
                <Button
                  variant="secondary"
                  disabled={blocked || command.busy}
                  aria-pressed={selected === "copy"}
                  onClick={() =>
                    setActions((current) => ({ ...current, [preview.location_id]: "copy" }))
                  }
                >
                  Keep a separate copy
                </Button>
                <Button
                  variant="secondary"
                  disabled={blocked || command.busy}
                  aria-pressed={selected === "replace"}
                  onClick={() =>
                    setActions((current) => ({ ...current, [preview.location_id]: "replace" }))
                  }
                >
                  Replace this album
                </Button>
              </div>
              {selected && (
                <>
                  {selected === "skip" ? (
                    <p className="warning">
                      This skips the whole album request. It cancels the queued request and makes no
                      library changes.
                    </p>
                  ) : selected === "replace" ? (
                    <p className="warning">
                      The current files and extras will be retained in hidden recovery storage
                      before the new album becomes visible.
                    </p>
                  ) : (
                    <p>
                      The server will use a separate album folder named for this request. The
                      existing folder will remain unchanged.
                    </p>
                  )}
                  <Button
                    disabled={blocked || command.busy || isApproved}
                    onClick={() =>
                      void command.run(async () => {
                        const body: AlbumReplacementDecision = {
                          action: selected,
                          expected_manifest_sha256: preview.manifest_sha256,
                        };
                        const saved = await request<SavedReview>(
                          `/api/jobs/${preview.job_id}/plans/${preview.plan_version}/publication/replacement-review/${preview.location_id}`,
                          { body },
                        );
                        onApproved(saved);
                      })
                    }
                  >
                    {command.busy
                      ? "Saving decision…"
                      : selected === "skip"
                        ? "Confirm skip"
                        : selected === "copy"
                          ? "Confirm separate copy"
                          : "Confirm replacement"}
                  </Button>
                </>
              )}
            </div>
          </article>
        );
      })}
    </section>
  );
}
