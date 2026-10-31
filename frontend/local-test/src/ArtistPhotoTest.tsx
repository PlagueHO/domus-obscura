import { useState } from "react";
import type { ArtistPhotoState } from "../../src/generated/ArtistPhotoState";
import { ArtistPhotoView } from "../../app/ArtistPhoto";
import { Button } from "../../app/components/ui";

const svg =
  '<svg xmlns="http://www.w3.org/2000/svg" width="320" height="160" viewBox="0 0 320 160"><rect width="320" height="160" fill="#262036"/><circle cx="160" cy="58" r="28" fill="#b98cff"/><path d="M92 160a68 68 0 0 1 136 0" fill="#78628e"/></svg>';

const binding: Extract<ArtistPhotoState, { status: "ready" }>["binding"] = {
  id: 1,
  version: 1,
  photo: {
    artist: { provider: "musicbrainz", id: "01234567-89ab-cdef-0123-456789abcdef" },
    provider: "wikimedia-commons",
    source: {
      file: "File:Generated fixture.jpg",
      entity_id: "1",
      revision: "1",
      timestamp: "2026-01-01T00:00:00Z",
      sha1: null,
      bytes: 1000,
      width: 320,
      height: 160,
      mime: "image/jpeg",
      original_url: "https://example.invalid/generated",
      rights: {
        title: "Generated portrait illustration",
        creator: "Local fixture, not a real artist",
        credit: "Generated locally. No Commons file was downloaded.",
        license: "CC0 1.0",
        license_url: "https://creativecommons.org/publicdomain/zero/1.0/",
        source_url: "https://example.invalid/generated",
        notices: "Presentation sample only.",
      },
    },
    source_sha256: "a".repeat(64),
    original_sha256: "b".repeat(64),
    jpeg_sha256: "c".repeat(64),
    width: 320,
    height: 160,
    fetched_at: 1,
    expires_at: 9999999999,
  },
};

export const generatedPhoto: Extract<ArtistPhotoState, { status: "ready" }> = {
  status: "ready",
  image_url: `data:image/svg+xml,${encodeURIComponent(svg)}`,
  binding,
  photo: binding.photo,
};

export function ArtistPhotoTest() {
  const [mode, setMode] = useState("ready");
  const state: ArtistPhotoState =
    mode === "ready" || mode === "small"
      ? {
          ...generatedPhoto,
          image_url:
            mode === "small"
              ? `data:image/svg+xml,${encodeURIComponent(svg.replace('width="320" height="160"', 'width="80" height="40"'))}`
              : generatedPhoto.image_url,
          photo: {
            ...generatedPhoto.photo,
            width: mode === "small" ? 80 : 320,
            height: mode === "small" ? 40 : 160,
          },
        }
      : mode === "missing"
        ? { status: "missing" }
        : mode === "disabled"
          ? { status: "disabled" }
          : { status: "unbound" };
  return (
    <main className="library-test">
      <header className="page-heading">
        <div>
          <h1>Artist photo fixtures</h1>
          <p>
            Generated illustration and rights records only. No provider, API or library is
            connected.
          </p>
          <a href="/">Return to the local test bench</a>
        </div>
      </header>
      <div className="actions">
        {["ready", "small", "unbound", "missing", "disabled", "loading", "failed"].map((value) => (
          <Button
            key={value}
            variant={mode === value ? "default" : "secondary"}
            onClick={() => setMode(value)}
          >
            {value}
          </Button>
        ))}
      </div>
      <section className="hero artist-hero album-hero">
        <div>
          <ArtistPhotoView
            key={mode}
            title="Generated artist"
            state={state}
            loading={mode === "loading"}
            error={
              mode === "failed"
                ? new Error("Generated provider failure. No stale photo is shown.")
                : undefined
            }
            onRetry={() => setMode("ready")}
          />
        </div>
        <div>
          <h2>Generated artist</h2>
          <p>The photo keeps its aspect ratio and is never enlarged.</p>
        </div>
      </section>
    </main>
  );
}
