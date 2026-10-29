import { renderToStaticMarkup } from "react-dom/server";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useResource } from "./hooks";
import { LibraryBrowser, LibraryFileDetails, FolderRefresh } from "./LibraryBrowse";
import { LocalMatches } from "./LocalMatches";
import type { Location } from "../src/generated/Location";

vi.mock("./hooks", async (original) => ({
  ...(await original<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

vi.mock("react", async (original) => {
  const actual = await original<typeof import("react")>();
  return { ...actual, useState: vi.fn(actual.useState) };
});

const root: Location = {
  id: 1,
  kind: "main",
  root: "generated-root",
  coverage: "complete",
  observed_at: 1,
  last_error: null,
  native_identity: "generated-identity",
};
const coverage = {
  location_id: 1,
  coverage: "incomplete",
  observed_at: 1,
  last_error: "Drive unavailable",
  metadata_files: 2,
  metadata_issues: 1,
  unobserved_files: 3,
  folder_state: null,
  folder_observed_at: null,
  folder_error: null,
};
function resource(data: unknown, error?: Error, loading = false) {
  return { data, error, loading, refresh: vi.fn() };
}
beforeEach(() => vi.mocked(useResource).mockReset());

describe("indexed Library evidence", () => {
  it("keeps duplicate album folders visible and links local identifiers", () => {
    vi.mocked(useResource)
      .mockReturnValueOnce(
        resource([
          { id: 10, title: "Album", artist: "Artist", folder: "original", file_count: 2 },
          { id: 11, title: "Album", artist: "Artist", folder: "other", file_count: 3 },
        ]),
      )
      .mockReturnValueOnce(resource(undefined))
      .mockReturnValueOnce(resource(coverage));
    const markup = renderToStaticMarkup(
      <LibraryBrowser
        locations={[root]}
        location={1}
        view="albums"
        artist={undefined}
        album={undefined}
        entry={undefined}
        readOnly
      />,
    );
    expect(markup).toContain("original");
    expect(markup).toContain("other");
    expect(markup).toContain("album=10");
    expect(markup).toContain("album=11");
    expect(markup).toContain("Saved location coverage: incomplete");
    expect(markup).toContain("Drive unavailable");
    expect(markup).not.toContain("Start Download");
  });

  it("does not turn a failed album read into a no-match claim", () => {
    vi.mocked(useResource)
      .mockReturnValueOnce(resource([], new Error("Index offline")))
      .mockReturnValueOnce(resource(undefined))
      .mockReturnValueOnce(resource(coverage));
    const markup = renderToStaticMarkup(
      <LibraryBrowser
        locations={[root]}
        location={1}
        view="albums"
        artist={undefined}
        album={undefined}
        entry={undefined}
        readOnly
      />,
    );
    expect(markup).toContain("Index offline");
    expect(markup).not.toContain("No indexed albums on this page");
  });

  it("shows failed metadata without invented tags or an automatic JPEG read", () => {
    vi.mocked(useResource).mockReturnValue(
      resource({
        id: 1,
        relative_path: "unknown.mp3",
        present: true,
        observed_at: 1,
        metadata: {
          status: "invalid",
          error: "Malformed metadata",
          size: 12,
          tags: { artists: [], album_artists: [], identifiers: {}, embedded_cover: false },
          audio: { duration_ms: null },
        },
      }),
    );
    const markup = renderToStaticMarkup(<LibraryFileDetails location={1} entry={1} />);
    expect(markup).toContain("Malformed metadata");
    expect(markup).toContain("<dd>Unknown</dd>");
    expect(markup).not.toContain("<img");
    expect(markup).toContain("do not prove an exact edition");
  });

  it("keeps folder disk refresh explicit and respects read-only policy", () => {
    vi.mocked(useResource).mockReturnValue(resource(undefined));
    const markup = renderToStaticMarkup(<FolderRefresh location={1} folder="selected" readOnly />);
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Refresh folder from disk/);
    expect(markup).toContain("direct entries only");
    expect(markup).toContain("System settings are read-only");
    expect(markup).not.toContain("Folder refresh completed");
  });

  it("allows explicit recovery when saved folder state still says scanning", () => {
    vi.mocked(useResource)
      .mockReturnValueOnce(resource(undefined))
      .mockReturnValueOnce(resource({ ...coverage, folder_state: "scanning" }));
    const markup = renderToStaticMarkup(
      <FolderRefresh location={1} folder="selected" readOnly={false} />,
    );
    expect(markup).toContain("Saved folder state: scanning");
    expect(markup).not.toMatch(/<button[^>]*disabled[^>]*>Refresh folder from disk/);
  });

  it("reloads album members, assets and folder coverage with the index revision", () => {
    vi.mocked(useState)
      .mockReturnValueOnce([[0], vi.fn()])
      .mockReturnValueOnce([3, vi.fn()]);
    vi.mocked(useResource)
      .mockReturnValue(resource([]))
      .mockReturnValueOnce(resource([]))
      .mockReturnValueOnce(
        resource({ id: 10, title: "Album", artist: "Artist", folder: "selected", file_count: 1 }),
      )
      .mockReturnValueOnce(resource(coverage));
    renderToStaticMarkup(
      <LibraryBrowser
        locations={[root]}
        location={1}
        view="albums"
        artist={undefined}
        album={10}
        entry={undefined}
        readOnly={false}
      />,
    );
    const keys = vi.mocked(useResource).mock.calls.map((call) => call[1]);
    expect(keys).toContain("1:10:false:0:3");
    expect(keys).toContain("1:10:true:0:3");
    expect(keys).toContain("1:selected:3");
  });

  it.each([true, false])(
    "hides an open JPEG preview during a pending or failed reload (%s)",
    (loading) => {
      vi.mocked(useState).mockReturnValueOnce([true, vi.fn()]);
      vi.mocked(useResource).mockReturnValue(
        resource(
          {
            id: 1,
            relative_path: "cover.jpg",
            present: true,
            observed_at: 1,
            metadata: {
              status: "asset",
              tags: { artists: [], album_artists: [], identifiers: {} },
              audio: {},
            },
          },
          loading ? undefined : new Error("Reload failed"),
          loading,
        ),
      );
      const markup = renderToStaticMarkup(<LibraryFileDetails location={1} entry={1} />);
      expect(markup).toContain("Hide JPEG preview");
      expect(markup).not.toContain("<img");
    },
  );

  it("reloads saved file details with the index revision", () => {
    vi.mocked(useState)
      .mockReturnValueOnce([[0], vi.fn()])
      .mockReturnValueOnce([3, vi.fn()]);
    vi.mocked(useResource).mockReturnValue(resource(undefined));
    renderToStaticMarkup(
      <LibraryBrowser
        locations={[root]}
        location={1}
        view="file"
        artist={undefined}
        album={undefined}
        entry={5}
        readOnly={false}
      />,
    );
    expect(vi.mocked(useResource).mock.calls.map((call) => call[1])).toContain("1:5:3");
  });

  it("keeps main and backup match evidence separate and discloses its basis", () => {
    vi.mocked(useResource).mockReturnValue(
      resource({
        coverage,
        matches: [
          {
            query_index: 0,
            artist_id: 5,
            albums: [
              { id: 10, folder: "original" },
              { id: 11, folder: "other" },
            ],
          },
        ],
      }),
    );
    const markup = renderToStaticMarkup(
      <LocalMatches
        locations={[root, { ...root, id: 2, kind: "backup", root: "generated-backup" }]}
        queries={[{ artist: "Artist", album: "Album" }]}
      />,
    );
    expect(markup).toContain("Saved artist and album tags only");
    expect(markup).toContain("do not confirm an exact edition");
    expect(markup).toContain("backup");
    expect(markup).toContain("location=1");
    expect(markup).not.toContain("location=2");
    expect(markup).toContain("Browse all indexed artist albums");
  });

  it("labels retained failed match reads as stale rather than confirmed absence", () => {
    vi.mocked(useResource).mockReturnValue(resource(undefined, new Error("Match read failed")));
    const markup = renderToStaticMarkup(
      <LocalMatches locations={[root]} queries={[{ artist: "Artist", album: "Album" }]} />,
    );
    expect(markup).toContain("unavailable or stale");
    expect(markup).not.toContain("No indexed album match");
  });
});
