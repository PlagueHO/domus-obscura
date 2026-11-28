import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Location } from "../src/generated/Location";
import type { LibrarySetup } from "../src/generated/LibrarySetup";
import {
  LibraryFolderList,
  LibraryFolderRegistration,
  SetupWizard,
  libraryFolder,
  needsLibrarySetup,
} from "./LibrarySetup";
import { PrimaryLibrary } from "./PrimaryLibrary";

const primary: Location = {
  id: 1,
  root: "storage-root:main",
  display_path: "\\\\plagueserver\\Music",
  kind: "main",
  coverage: "complete",
  observed_at: 1,
  last_error: null,
  native_identity: "fixture",
};
const setup: LibrarySetup = {
  primary_id: 1,
  primary_error: null,
  output_configured: true,
  folders: [
    { root_id: "main", display_path: "\\\\plagueserver\\Music", paired: false, location_id: 1 },
  ],
};

describe("library setup", () => {
  it("shows a primary location control with explicit restart and no-media-change terms", () => {
    const markup = renderToStaticMarkup(
      <PrimaryLibrary hasPrimary disabled={false} onChanged={() => {}} />,
    );
    expect(markup).toContain("Change primary library");
    expect(markup).toContain("Change…");
    expect(markup).toContain("Restarts the API and worker. No media is moved.");
    expect(markup).not.toContain("Save primary library change");
  });
  it("shows the folder entry directly when there is no primary", () => {
    const markup = renderToStaticMarkup(
      <PrimaryLibrary hasPrimary={false} disabled={false} onChanged={() => {}} />,
    );
    expect(markup).toContain("Primary library folder");
    expect(markup).toContain("Browse…");
  });
  it("uses the saved path and plain role instead of an internal storage key", () => {
    expect(libraryFolder(primary)).toBe("\\\\plagueserver\\Music");
    expect(libraryFolder({ ...primary, display_path: "\\\\?\\UNC\\server\\Music" })).toBe(
      "\\\\server\\Music",
    );
    const markup = renderToStaticMarkup(<LibraryFolderList locations={[primary]} />);
    expect(markup).toContain("<h3>Primary library</h3>");
    expect(markup).toContain("Last full scan completed");
    expect(markup).not.toContain("Bound root");
    expect(markup).not.toContain("<strong>storage-root:");
  });

  it("registers only selected folders and offers no path field or separate backup scan", () => {
    const markup = renderToStaticMarkup(
      <LibraryFolderRegistration
        setup={{
          ...setup,
          folders: [
            ...setup.folders,
            { root_id: "copy", display_path: "D:\\Backup", paired: false, location_id: null },
          ],
        }}
        locations={[primary]}
        disabled={false}
        onChanged={() => {}}
      />,
    );
    expect(markup).toContain("D:\\Backup");
    expect(markup).toContain("Add backup library");
    expect(markup).not.toContain("<input");
    expect(markup).not.toContain('value="false:main"');
    expect(markup).not.toContain(">Scan ");
    expect(markup).toContain("Stop the API and workers");
  });

  it("requires a verified primary and output policy, not provider accounts or a backup", () => {
    expect(needsLibrarySetup(setup)).toBe(false);
    expect(needsLibrarySetup({ ...setup, primary_id: null })).toBe(true);
    expect(needsLibrarySetup({ ...setup, primary_error: "offline" })).toBe(true);
    expect(needsLibrarySetup({ ...setup, output_configured: false })).toBe(true);
  });

  it("retains an offline primary and blocks continuation rather than offering replacement", () => {
    const markup = renderToStaticMarkup(
      <SetupWizard
        setup={{ ...setup, primary_error: "The share is offline." }}
        locations={[primary]}
        readOnly={false}
        loading={false}
        error={undefined}
        onChanged={() => {}}
      />,
    );
    expect(markup).toContain("The share is offline.");
    expect(markup).toContain("Restore access to the saved folder");
    expect(markup).not.toContain("Use as primary library");
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Continue to output policy/);
  });
});
