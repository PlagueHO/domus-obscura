import { useEffect, useRef, useState } from "react";
import type { ArtistPhotoState } from "../src/generated/ArtistPhotoState";
import type { ImageRights } from "../src/generated/ImageRights";
import { request, query } from "./api";
import { useMetadataResource } from "./hooks";
import { Alert, Button, Cover } from "./components/ui";

export function PhotoCredit({ rights }: { rights: ImageRights }) {
  return (
    <details className="artist-photo-credit">
      <summary>Photo credit · {rights.license}</summary>
      <p>
        {rights.title} — {rights.creator}
      </p>
      {rights.credit && <p>{rights.credit}</p>}
      {rights.notices && <p>{rights.notices}</p>}
      <p>
        <a href={rights.source_url} target="_blank" rel="noreferrer">
          Photo source
        </a>
        {" · "}
        <a href={rights.license_url} target="_blank" rel="noreferrer">
          {rights.license}
        </a>
      </p>
      <p>Normalized and resized without cropping. The source license also applies to this image.</p>
    </details>
  );
}

export function ArtistPhoto({ id, title }: { id: string; title: string }) {
  const element = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!element.current) return;
    if (typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(element.current);
    return () => observer.disconnect();
  }, []);
  const photo = useMetadataResource<ArtistPhotoState>(
    (signal) =>
      visible
        ? request(
            `/api/artist-photos?${query({ artist_provider: "musicbrainz", artist_id: id })}`,
            { signal },
          )
        : Promise.resolve({ status: "disabled" }),
    `${id}:${visible}`,
  );
  return (
    <div ref={element}>
      <ArtistPhotoView
        key={id}
        title={title}
        state={visible ? photo.data : undefined}
        loading={visible && photo.loading}
        error={photo.error}
        onRetry={photo.refresh}
      />
    </div>
  );
}

/** Presentation only. The separate browser harness supplies generated evidence. */
export function ArtistPhotoView({
  title,
  state,
  loading = false,
  error,
  onRetry,
}: {
  title: string;
  state?: ArtistPhotoState | undefined;
  loading?: boolean;
  error?: unknown;
  onRetry: () => void;
}) {
  const [failedUrl, setFailedUrl] = useState<string>();
  const current = !loading && !error ? state : undefined;
  const ready = current?.status === "ready" ? current : undefined;
  const failed = ready && failedUrl === ready.image_url;
  const missing =
    current?.status === "unbound"
      ? "No photo is linked to this artist."
      : current?.status === "missing"
        ? "The linked photo is missing from Commons."
        : current?.status === "disabled"
          ? "Artist photos are disabled."
          : undefined;
  return (
    <figure className="artist-photo">
      <div className="artist-photo-media">
        {ready && !failed ? (
          <img
            src={ready.image_url}
            width={ready.photo.width}
            height={ready.photo.height}
            alt={`Photo of ${title}`}
            loading="lazy"
            onError={() => setFailedUrl(ready.image_url)}
          />
        ) : (
          <Cover title={title} />
        )}
      </div>
      {loading && <p role="status">Loading artist photo…</p>}
      {missing && <p className="muted">{missing}</p>}
      <Alert error={error} />
      {failed && (
        <p role="alert">The photo could not be loaded. Reload its current source details.</p>
      )}
      {(Boolean(error) || failed) && (
        <Button
          variant="ghost"
          onClick={() => {
            setFailedUrl(undefined);
            onRetry();
          }}
        >
          Reload photo
        </Button>
      )}
      {ready && !failed && (
        <figcaption>
          <PhotoCredit rights={ready.photo.source.rights} />
        </figcaption>
      )}
    </figure>
  );
}
