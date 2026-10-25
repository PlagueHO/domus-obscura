import { useState } from "react";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { ApproveArtwork } from "../src/generated/ApproveArtwork";
import { request } from "./api";
import { useCommand } from "./hooks";
import { Alert, Button, Check, Empty } from "./components/ui";

export function ArtworkReview({
  base,
  candidates,
  selected,
  onSelect,
  refresh,
}: {
  base: string;
  candidates: ArtworkCandidate[];
  selected: ArtworkCandidate | undefined;
  onSelect: (id: number) => void;
  refresh: () => void;
}) {
  const [permitted, setPermitted] = useState(false);
  const [exact, setExact] = useState(false);
  const [loaded, setLoaded] = useState<string>();
  const [imageError, setImageError] = useState(false);
  const command = useCommand();
  async function upload(file: File | undefined) {
    if (!file || !permitted) return;
    if (file.size === 0 || file.size > 8 * 1024 * 1024)
      throw new Error("Choose an image larger than zero bytes and at most 8 MiB.");
    const candidate = await request<ArtworkCandidate>(
      `${base}/artwork/local?acknowledge_permitted_use=true`,
      { file },
    );
    onSelect(candidate.id);
    setExact(false);
    setPermitted(false);
    refresh();
  }
  async function approve() {
    if (!selected || !exact || loaded !== selected.sha256) return;
    const body: ApproveArtwork = { candidate_id: selected.id, sha256: selected.sha256 };
    await request(`${base}/artwork/approve`, { body });
    setExact(false);
    refresh();
  }
  return (
    <section className="review-section" aria-labelledby="cover-heading">
      <h2 id="cover-heading">Choose the cover</h2>
      <p>
        A local image keeps local provenance. Permission to use an image does not approve its
        normalized JPEG preview.
      </p>
      <Check checked={permitted} onChange={setPermitted} disabled={command.busy}>
        I have permission to use the image I will upload.
      </Check>
      <label className="file-label">
        Upload a local cover · maximum 8 MiB
        <input
          type="file"
          accept="image/jpeg,image/png,image/webp"
          disabled={command.busy || !permitted}
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            void command.run(() => upload(file));
          }}
        />
      </label>
      <Alert error={command.error} />
      {candidates.length === 0 && (
        <Empty>
          No cover candidates yet. Upload a permitted cover or prepare one with the existing artwork
          commands.
        </Empty>
      )}
      {candidates.length > 0 && (
        <fieldset className="cover-choices" disabled={command.busy}>
          <legend>Saved cover candidates</legend>
          <div className="cover-options">
            {candidates.map((candidate) => (
              <label className="cover-option" key={candidate.id}>
                <input
                  type="radio"
                  name="cover-candidate"
                  value={candidate.id}
                  checked={selected?.id === candidate.id}
                  onChange={() => {
                    onSelect(candidate.id);
                    setExact(false);
                    setLoaded(undefined);
                    setImageError(false);
                  }}
                />
                <img
                  src={`${base}/artwork/${candidate.id}/image`}
                  alt={`Cover candidate ${candidate.id}`}
                  loading="lazy"
                />
                <span>
                  #{candidate.id} · {candidate.source}
                  <small>
                    {candidate.approved_at !== null ? "Approved exact JPEG" : "Not approved"}
                  </small>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
      )}
      {selected && (
        <>
          <h3 id="selected-cover-heading">Selected exact JPEG preview</h3>
          <div className="artwork-review" aria-labelledby="selected-cover-heading">
            <img
              key={selected.id}
              className="cover-preview"
              src={`${base}/artwork/${selected.id}/image`}
              alt="Exact normalized JPEG cover for approval"
              onLoad={() => {
                setLoaded(selected.sha256);
                setImageError(false);
              }}
              onError={() => {
                setLoaded(undefined);
                setImageError(true);
              }}
            />
            <div>
              <p>
                {selected.width} × {selected.height} · {selected.bytes.toLocaleString()} bytes ·{" "}
                {selected.source}
              </p>
              <p className="identifier">JPEG SHA-256 · {selected.sha256}</p>
              {imageError && (
                <p className="warning">
                  The exact JPEG preview could not load. Refresh the request before approval.
                </p>
              )}
              {selected.approved_at !== null ? (
                <p className="success">This exact cover approval is saved.</p>
              ) : (
                <>
                  <Check
                    checked={exact}
                    onChange={setExact}
                    disabled={command.busy || loaded !== selected.sha256}
                  >
                    I reviewed this exact normalized JPEG preview.
                  </Check>
                  <Button
                    disabled={command.busy || !exact || loaded !== selected.sha256}
                    onClick={() => void command.run(approve)}
                  >
                    Approve this exact cover
                  </Button>
                </>
              )}
            </div>
          </div>
        </>
      )}
    </section>
  );
}
