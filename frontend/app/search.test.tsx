import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Search, nextArtistOffset } from "./Search";
import { EditionChoices, editionLabel } from "./Catalog";

afterEach(() => vi.useRealTimers());

describe("grouped provider search", () => {
  it("keeps the exact current edition in a paged dropdown without inventing approval or ranking", () => {
    const current = {
      id: "current-id",
      title: "Exact <edition>",
      artist: "Generated artist",
      date: "2024",
      country: "NZ",
      disambiguation: "First issue",
      status: "Official",
      group: null,
    };
    const other = { ...current, id: "other-id", disambiguation: "Reissue" };
    const markup = renderToStaticMarkup(
      <EditionChoices current={current} items={[other]} disabled />,
    );
    expect(markup).toContain('value="current-id" selected=""');
    expect(markup).toContain('value="other-id"');
    expect(markup).toContain("Exact &lt;edition&gt;");
    expect(markup).toContain("disabled");
    expect(markup).not.toContain("Recommended");
    expect(markup).not.toContain("Approve");
    expect(editionLabel(other)).toContain("Reissue");
    const samePage = renderToStaticMarkup(
      <EditionChoices current={current} items={[current, other]} disabled={false} />,
    );
    expect(samePage.match(/value="current-id"/g)).toHaveLength(1);
  });

  it("keeps both groups visible and does not turn an empty query into a provider call", () => {
    const markup = renderToStaticMarkup(<Search term="Generated artist" />);
    expect(markup).toContain('id="artist-results-heading"');
    expect(markup).toContain("Albums and exact editions");
    expect(markup).toContain("Album search follows the first artist request");
    expect(markup).not.toContain("Start Download");
    expect(renderToStaticMarkup(<Search term="  " />)).toContain("Enter an artist or album");
  });

  it("uses the provider cursor rather than assuming a full artist page", () => {
    const artist = {
      id: "artist",
      name: "Generated artist",
      sort_name: null,
      disambiguation: null,
      country: null,
      kind: null,
    };
    expect(nextArtistOffset({ offset: 24, count: 40, items: [artist] })).toBe(25);
    expect(nextArtistOffset({ offset: 39, count: 40, items: [artist] })).toBeNull();
    expect(nextArtistOffset({ offset: 24, count: 40, items: [] })).toBeNull();
  });
});
