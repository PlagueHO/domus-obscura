import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { JobSummary } from "../src/generated/JobSummary";
import type { PublicationDestination } from "../src/generated/PublicationDestination";
import {
  AlbumReplacementRestore,
  PublicationRecovery,
  retainedPublicationRun,
} from "./PublicationRecovery";

const job: JobSummary = {
  id: 3,
  plan_version: 2,
  state: "backup_pending",
  created_at: 1,
  album: "Generated album",
  album_artist: "Generated artist",
  approved_at: 1,
  artwork_candidate_id: null,
};
const target: PublicationDestination = {
  id: 1,
  job_id: 3,
  plan_version: 2,
  run_id: 7,
  location_id: 1,
  root: "generated-root",
  root_identity: "generated-id",
  kind: "main",
  relative_folder: "Generated album",
  manifest_sha256: "a".repeat(64),
  album_identity: "generated-album-id",
  verified_at: 1,
  last_error: null,
  replacement: null,
};

const command = {
  busy: false,
  error: undefined,
  run: async (action: () => Promise<void>) => action(),
};

describe("retained publication recovery", () => {
  it("uses the shared request command lock while another command is pending", () => {
    const markup = renderToStaticMarkup(
      <PublicationRecovery
        job={job}
        version={2}
        destinations={[target]}
        status={{ enabled: true, busy: false, last_error: null }}
        blocked={false}
        command={{ ...command, busy: true }}
        onChanged={() => {}}
      />,
    );
    expect(markup).toMatch(/<button[^>]*disabled/);
  });
  it("uses one exact retained journal only in publication states", () => {
    for (const state of ["publishing", "backup_pending"]) {
      expect(retainedPublicationRun({ ...job, state }, 2, [target])).toBe(7);
    }
    for (const state of ["queued", "downloading", "awaiting_input", "complete", "cancelled"]) {
      expect(retainedPublicationRun({ ...job, state }, 2, [target])).toBeUndefined();
    }
    expect(retainedPublicationRun(job, 3, [target])).toBeUndefined();
    expect(retainedPublicationRun(job, 2, [])).toBeUndefined();
    for (const changed of [{ job_id: 4 }, { plan_version: 3 }, { run_id: 0 }, { run_id: 8 }]) {
      expect(retainedPublicationRun(job, 2, [target, { ...target, ...changed }])).toBeUndefined();
    }
  });

  it("requires a separate confirmation and describes the retained-file limits", () => {
    const markup = renderToStaticMarkup(
      <PublicationRecovery
        command={command}
        job={job}
        version={2}
        destinations={[target]}
        status={{ enabled: true, busy: false, last_error: null }}
        blocked={false}
        onChanged={() => {}}
      />,
    );
    expect(markup).toContain("Review publication recovery");
    expect(markup).not.toContain("Confirm new-only publication");
    expect(markup).toContain("does not download or normalize again");
    expect(markup).toContain("Partial, different or missing receipted files remain errors");
    expect(markup).not.toMatch(/<button[^>]*disabled/);
  });

  it("blocks unavailable, busy or failed reads without hiding retained evidence", () => {
    for (const [status, blocked] of [
      [undefined, false],
      [{ enabled: false, busy: false, last_error: null }, false],
      [{ enabled: true, busy: true, last_error: null }, false],
      [{ enabled: true, busy: false, last_error: null }, true],
    ] as const) {
      const markup = renderToStaticMarkup(
        <PublicationRecovery
          command={command}
          job={job}
          version={2}
          destinations={[target]}
          status={status}
          blocked={blocked}
          onChanged={() => {}}
        />,
      );
      expect(markup).toMatch(/<button[^>]*disabled/);
      expect(markup).toContain("normalization run 7");
    }
  });

  it("offers a state-bound restore only after publication is complete", () => {
    const destination: PublicationDestination = {
      ...target,
      replacement: {
        review_id: 11,
        original_folder: "Generated album",
        original_album_identity: "old-album",
        original_files: [
          { name: "folder.jpg", bytes: 9, sha256: "b".repeat(64) },
          { name: "manual.txt", bytes: 11, sha256: "c".repeat(64) },
        ],
        retained_folder: ".domus-retained-3-2-1-11",
        restore_folder: ".domus-restore-3-2-1-11",
        state: "published",
      },
    };
    const markup = renderToStaticMarkup(
      <AlbumReplacementRestore
        job={{ ...job, state: "complete" }}
        version={2}
        destinations={[destination]}
        blocked={false}
        command={command}
        onChanged={() => {}}
      />,
    );
    expect(markup).toContain("Review restore");
    expect(markup).toContain("manual.txt");
    expect(markup).toContain(destination.manifest_sha256);
    expect(
      renderToStaticMarkup(
        <AlbumReplacementRestore
          job={job}
          version={2}
          destinations={[destination]}
          blocked={false}
          command={command}
          onChanged={() => {}}
        />,
      ),
    ).toBe("");
  });
});
