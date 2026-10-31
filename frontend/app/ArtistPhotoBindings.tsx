import { useRef, useState } from "react";
import type { ArtistPhotoBinding } from "../src/generated/ArtistPhotoBinding";
import type { ArtistPhotoCheck } from "../src/generated/ArtistPhotoCheck";
import type { CheckArtistPhoto } from "../src/generated/CheckArtistPhoto";
import type { SaveArtistPhoto } from "../src/generated/SaveArtistPhoto";
import { request, query } from "./api";
import { useCommand, useMetadataResource } from "./hooks";
import { Alert, Button, Check, Pager } from "./components/ui";
import { PhotoCredit } from "./ArtistPhoto";

export function ArtistPhotoBindings({
  readOnly,
  enabled,
}: {
  readOnly: boolean;
  enabled: boolean;
}) {
  const initial =
    new URLSearchParams(typeof location === "undefined" ? "" : location.hash.split("?")[1]).get(
      "artist",
    ) ?? "";
  const [artistId, setArtistId] = useState(initial);
  const [file, setFile] = useState("File:");
  const [checked, setChecked] = useState<ArtistPhotoCheck>();
  const [permitted, setPermitted] = useState(false);
  const [previewFailed, setPreviewFailed] = useState(false);
  const [previewLoaded, setPreviewLoaded] = useState(false);
  const [message, setMessage] = useState("");
  const [pages, setPages] = useState([0]);
  const revision = useRef(0);
  const after = pages.at(-1) ?? 0;
  const bindings = useMetadataResource(
    (signal) =>
      request<ArtistPhotoBinding[]>(`/api/system/artist-photos?${query({ after, limit: 50 })}`, {
        signal,
      }),
    String(after),
  );
  const command = useCommand();
  const invalidate = () => {
    revision.current++;
    setChecked(undefined);
    setPermitted(false);
    setPreviewFailed(false);
    setPreviewLoaded(false);
    setMessage("");
  };
  const selection = (): CheckArtistPhoto => ({
    artist: { provider: "musicbrainz", id: artistId.trim() },
    provider: "wikimedia-commons",
    file: file.trim(),
  });
  const inspect = () =>
    command.run(async () => {
      invalidate();
      const current = revision.current;
      const result = await request<ArtistPhotoCheck>("/api/system/artist-photos/check", {
        body: selection(),
      });
      if (current === revision.current) setChecked(result);
    });
  const save = () =>
    command.run(async () => {
      if (!checked || !permitted || !previewLoaded || previewFailed)
        throw new Error("Check the exact photo and confirm its use before saving.");
      const current = revision.current;
      const body: SaveArtistPhoto = {
        selection: selection(),
        expected_version: checked.expected_version,
        expected_confirmation: checked.confirmation,
        acknowledge_permitted_use: permitted,
      };
      await request("/api/system/artist-photos/save", { body });
      bindings.refresh();
      if (current === revision.current) {
        setChecked(undefined);
        setPermitted(false);
        setMessage("Artist photo link saved. Later artist reads use this link.");
      }
    });
  const remove = (binding: ArtistPhotoBinding) =>
    command.run(async () => {
      await request("/api/system/artist-photos/remove", {
        body: { id: binding.id, expected_version: binding.version },
      });
      invalidate();
      bindings.refresh();
      setMessage("Artist photo link removed. No library media was changed.");
    });
  return (
    <section className="artist-photo-bindings" aria-labelledby="artist-photo-bindings-title">
      <h3 id="artist-photo-bindings-title">Artist photo links</h3>
      <p>
        Link an exact MusicBrainz artist to one Commons File title. Names alone do not establish
        identity.
      </p>
      {!enabled && (
        <p>
          Activate Wikimedia Commons with a shared file cache before checking or saving a photo.
        </p>
      )}
      {enabled && (
        <p>
          If Commons returns an unsupported retry delay, requests stay paused.{" "}
          <Button
            variant="ghost"
            disabled={readOnly || command.busy}
            onClick={() =>
              void command.run(async () => {
                await request("/api/system/artist-photos/resume", { body: {} });
                setMessage(
                  "The unsupported Commons delay was cleared. Retry the original photo action.",
                );
              })
            }
          >
            Resume manually paused requests
          </Button>
        </p>
      )}
      <Alert error={command.error} />
      {message && <p role="status">{message}</p>}
      <fieldset disabled={readOnly || !enabled || command.busy}>
        <legend>Check and save a photo</legend>
        <label>
          MusicBrainz artist ID
          <input
            value={artistId}
            spellCheck={false}
            onChange={(event) => {
              invalidate();
              setArtistId(event.target.value);
            }}
          />
        </label>
        <label>
          Commons File title
          <input
            value={file}
            spellCheck={false}
            onChange={(event) => {
              invalidate();
              setFile(event.target.value);
            }}
          />
          <small>
            Enter a File: title, not a URL. JPEG and PNG originals must fit the image limits.
          </small>
        </label>
        <Button
          variant="secondary"
          disabled={!artistId.trim() || file.trim() === "File:"}
          onClick={() => void inspect()}
        >
          {command.busy ? "Working…" : "Check photo"}
        </Button>
        {checked && (
          <div className="artist-photo-check">
            <p>
              <strong>Artist:</strong> {checked.photo.artist.id}
            </p>
            <p>
              <strong>File:</strong> {checked.photo.source.file}
            </p>
            <img
              key={checked.confirmation}
              className="artist-photo-preview"
              src={checked.preview_url}
              width={checked.photo.width}
              height={checked.photo.height}
              alt="Selected artist photo preview"
              onLoad={() => setPreviewLoaded(true)}
              onError={() => {
                setPreviewLoaded(false);
                setPreviewFailed(true);
                setPermitted(false);
              }}
            />
            {previewFailed && (
              <p role="alert">The preview could not be loaded. Check the photo again.</p>
            )}
            <p>
              {checked.photo.width} × {checked.photo.height} pixels · checked source, not
              album-cover approval
            </p>
            <PhotoCredit rights={checked.photo.source.rights} />
            <p>
              Commons does not guarantee rights statements. Other rights can apply, including
              personality and privacy rights.
            </p>
            <Check
              checked={permitted}
              disabled={!previewLoaded || previewFailed}
              onChange={setPermitted}
            >
              This photo shows the exact artist. I have permission to use it under the shown license
              and other applicable rights.
            </Check>
            <Button
              disabled={!permitted || !previewLoaded || previewFailed}
              onClick={() => void save()}
            >
              Save artist photo link
            </Button>
          </div>
        )}
      </fieldset>
      <h4>Saved photo links</h4>
      <Alert error={bindings.error} />
      {bindings.loading && <p role="status">Loading saved photo links…</p>}
      {bindings.data?.length === 0 && <p>No artist photos are linked.</p>}
      {bindings.data?.map((binding) => (
        <div className="artist-photo-binding" key={binding.id}>
          <p>
            <a href={`#/artist/${encodeURIComponent(binding.photo.artist.id)}`}>
              {binding.photo.artist.id}
            </a>
            {" · "}
            {binding.photo.source.file}
          </p>
          <p>
            {binding.photo.provider} · version {binding.version}
          </p>
          <div className="actions">
            <Button
              variant="secondary"
              disabled={readOnly || command.busy}
              onClick={() => {
                invalidate();
                setArtistId(binding.photo.artist.id);
                setFile(binding.photo.source.file);
              }}
            >
              Edit link
            </Button>
            <Button
              variant="ghost"
              disabled={readOnly || command.busy}
              onClick={() => void remove(binding)}
            >
              Remove link
            </Button>
          </div>
        </div>
      ))}
      <Pager
        busy={bindings.loading || command.busy}
        previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
        next={
          bindings.data?.length === 50
            ? () => setPages([...pages, bindings.data?.at(-1)?.id ?? after])
            : undefined
        }
      />
    </section>
  );
}
