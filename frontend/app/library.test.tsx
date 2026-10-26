import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Location } from "../src/generated/Location";
import type { Entry } from "../src/generated/Entry";
import { useResource } from "./hooks";
import { Library, RootAvailability, entryName, parentFolder } from "./Library";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

const root: Location = {
  id: 1,
  kind: "main",
  root: "generated-root",
  coverage: "complete",
  observed_at: 1,
  last_error: null,
  native_identity: "generated-identity",
};
const entry: Entry = {
  id: 1,
  relative_path: "Artist/Album",
  kind: "directory",
  size: null,
  modified_at: null,
  present: true,
  scan_id: 1,
};
const refresh = vi.fn();

function render(locations: Location[], data: Entry[] | undefined, error?: Error, loading = false) {
  vi.mocked(useResource).mockReturnValue({ data, error, loading, refresh });
  return renderToStaticMarkup(<Library locations={locations} />);
}

beforeEach(() => vi.mocked(useResource).mockReset());

describe("read-only library", () => {
  it("requires an explicit root check and does not equate indexed coverage with reachability", () => {
    const markup = renderToStaticMarkup(<RootAvailability id={1} />);
    expect(markup).toContain("Check root availability");
    expect(markup).toContain("Root availability is not confirmed");
    expect(markup).toContain("does not scan folders");
    expect(markup).not.toContain("was available on the last check");
  });
  it("uses library identifiers, not native path rules, for folder navigation", () => {
    expect(parentFolder("Artist/Album")).toBe("Artist");
    expect(parentFolder("Artist")).toBe("");
    expect(parentFolder("")).toBe("");
    expect(entryName("Artist/Album")).toBe("Album");
    expect(entryName("Artist/Album/Track.mp3")).toBe("Track.mp3");
    expect(entryName("Artist")).toBe("Artist");
  });

  it("links setup without suggesting that browsing scans or changes media", () => {
    const markup = render([], []);
    expect(markup).toContain("No library roots");
    expect(markup).toContain('href="#/settings"');
    expect(markup).toContain("explicit scan");
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Refresh index view/);
  });

  it("shows bounded index entries and keeps confirmed absence distinct", () => {
    const markup = render(
      [root],
      [entry, { ...entry, id: 2, relative_path: "Artist/Track.mp3", kind: "file", present: false }],
    );
    expect(markup).toContain(">Album</button>");
    expect(markup).toContain(">Track.mp3</strong>");
    expect(markup).toContain("Not present in last complete observation");
    expect(markup).not.toContain("No indexed entries");
    expect(markup).not.toContain("Delete");
    expect(markup).not.toContain("Start Download");
  });

  it("does not turn failed or pending reads into an empty folder claim", () => {
    const failed = render([root], [], new Error("Index unavailable"));
    expect(failed).toContain("Index unavailable");
    expect(failed).toContain("not a current filesystem check");
    expect(failed).not.toContain("No indexed entries");
    expect(render([root], [], undefined, true)).not.toContain("No indexed entries");
    const empty = render([root], []);
    expect(empty).toContain("No indexed entries");
    expect(empty).toContain("Incomplete coverage does not confirm");
  });

  it("reports incomplete coverage and scan errors without changing them", () => {
    const markup = render(
      [{ ...root, coverage: "unknown", observed_at: null, last_error: "Drive unavailable" }],
      [],
    );
    expect(markup).toContain("No confirmed observation");
    expect(markup).toContain("Drive unavailable");
    expect(markup).toContain("Missing entries do not prove");
    expect(markup).toContain("does not scan the disk");
  });
});
