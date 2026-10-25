import { useState } from "react";
import type { ArtworkCandidate } from "../src/generated/ArtworkCandidate";
import type { ApproveArtwork } from "../src/generated/ApproveArtwork";
import type { CaaCoverSearch } from "../src/generated/CaaCoverSearch";
import type { PrepareCaaArtwork } from "../src/generated/PrepareCaaArtwork";
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
  const [search, setSearch] = useState<CaaCoverSearch>();
  const [imageId, setImageId] = useState("");
  const [providerPermitted, setProviderPermitted] = useState(false);
  const [background, setBackground] = useState<"original" | "black" | "white">("original");
  const command = useCommand();
  async function findCovers() {
    const result = await request<CaaCoverSearch>(`${base}/artwork/caa/search`, { method: "POST" });
    setSearch(result);
    setImageId(
      result.images.find((image) => image.front)?.image_id ?? result.images[0]?.image_id ?? "",
    );
    setProviderPermitted(false);
  }
  async function downloadCover() {
    if (!imageId || !providerPermitted) return;
    const body: PrepareCaaArtwork = {
      image_id: imageId,
      background:
        background === "original" ? null : background === "black" ? [0, 0, 0] : [255, 255, 255],
      acknowledge_permitted_use: true,
    };
    const candidate = await request<ArtworkCandidate>(`${base}/artwork/caa/download`, { body });
    onSelect(candidate.id);
    setExact(false);
    setLoaded(undefined);
    setImageError(false);
    setProviderPermitted(false);
    refresh();
  }
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
        Find Cover Art Archive images for this exact release. Download a selected image to create a
        normalized preview. Provider edit approval does not grant permission or approve your cover.
      </p>
      <Button
        variant="secondary"
        disabled={command.busy}
        onClick={() => void command.run(findCovers)}
      >
        {command.busy ? "Working on cover…" : "Find covers for this edition"}
      </Button>
      {search && search.images.length === 0 && (
        <Empty>
          No Cover Art Archive images were found for this exact edition. You can upload a permitted
          local cover below.
        </Empty>
      )}
      {search && search.images.length > 0 && (
        <fieldset disabled={command.busy}>
          <legend>Cover Art Archive suggestions</legend>
          <label>
            Image for this edition
            <select
              value={imageId}
              onChange={(event) => {
                setImageId(event.target.value);
                setProviderPermitted(false);
              }}
            >
              {search.images.map((image) => (
                <option key={image.image_id} value={image.image_id}>
                  {image.front ? "Front cover" : "Other artwork"} · {image.image_id}
                  {image.provider_approved === true ? " · Provider edit approved" : ""}
                </option>
              ))}
            </select>
          </label>
          <label>
            Background for transparent images
            <select
              value={background}
              onChange={(event) => {
                const value = event.target.value;
                if (value === "original" || value === "black" || value === "white")
                  setBackground(value);
              }}
            >
              <option value="original">No background selected</option>
              <option value="black">Black</option>
              <option value="white">White</option>
            </select>
          </label>
          <Check checked={providerPermitted} onChange={setProviderPermitted}>
            I have permission to use the selected Cover Art Archive image.
          </Check>
          <Button
            disabled={command.busy || !providerPermitted || !imageId}
            onClick={() => void command.run(downloadCover)}
          >
            Download cover preview
          </Button>
          <p>Downloaded previews appear below. Each exact JPEG needs separate approval.</p>
        </fieldset>
      )}
      <h3>Or upload a local cover</h3>
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
          No saved cover previews yet. Find a cover for this edition or upload a permitted image.
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
