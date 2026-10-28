import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, expect, it, vi } from "vitest";
import { SharingFolders } from "./SharingFolders";
import { useResource } from "./hooks";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

const location = {
  id: 1,
  root: "generated-library",
  kind: "main",
  coverage: "complete",
  observed_at: null,
  last_error: null,
  native_identity: "location-identity",
};
const binding = {
  location_id: 1,
  relative_path: "Artist",
  native_identity: "folder-identity",
  binding_id: 5,
  bound: true,
};

beforeEach(() => vi.mocked(useResource).mockReset());

function resources(failure?: Error, loading = false) {
  vi.mocked(useResource)
    .mockReturnValueOnce({ data: [location], error: undefined, loading: false, refresh: vi.fn() })
    .mockReturnValueOnce({ data: [binding], error: failure, loading, refresh: vi.fn() });
}

it("selects registered bindings and public aliases without a native folder input", () => {
  resources();
  const markup = renderToStaticMarkup(
    <SharingFolders
      roots={[{ binding_id: 5, alias: "Public Music" }]}
      disabled={false}
      onChange={vi.fn()}
    />,
  );
  expect(markup).toContain("Public Music");
  expect(markup).toContain("Bound folder: Artist");
  expect(markup).toContain("main · generated-library");
  expect(markup).toContain("Inspect folder");
  expect(markup).toContain("Bind inspected folder");
  expect(markup).toContain("Library-relative subfolder");
  expect(markup).toContain("First library locations");
  expect(markup).toContain("Next bound folders");
  expect(markup).not.toContain('value="folder-identity"');
  expect(markup).not.toContain('name="native_path"');
});

it("does not present retained binding data as current after a failed or pending refresh", () => {
  for (const [failure, loading] of [
    [new Error("Refresh failed"), false],
    [undefined, true],
  ] as const) {
    resources(failure, loading);
    const markup = renderToStaticMarkup(
      <SharingFolders
        roots={[{ binding_id: 5, alias: "Public Music" }]}
        disabled={false}
        onChange={vi.fn()}
      />,
    );
    expect(markup).not.toContain("Bound folder: Artist");
    expect(markup).toContain("Binding not checked on this page");
    expect(markup).not.toContain("Binding 5 · location");
  }
});

it("disables binding and selection commands when configuration editing is blocked", () => {
  resources();
  const markup = renderToStaticMarkup(<SharingFolders roots={[]} disabled onChange={vi.fn()} />);
  expect(markup).toMatch(/<button[^>]*disabled[^>]*>Inspect folder/);
  expect(markup).toMatch(/<button[^>]*disabled[^>]*>Bind inspected folder/);
  expect(markup).toMatch(/<input[^>]*type="checkbox"[^>]*disabled/);
});
