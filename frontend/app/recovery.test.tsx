import { renderToStaticMarkup } from "react-dom/server";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RecoveryEntry } from "../src/generated/RecoveryEntry";
import type { RecoveryPreview } from "../src/generated/RecoveryPreview";
import type { RecoveryOperation } from "../src/generated/RecoveryOperation";
import { useResource } from "./hooks";
import {
  Recovery,
  RecoveryDetail,
  RecoveryReview,
  continueRecoveryEntry,
  deleteRecoveryEntry,
  recoveryJobFilter,
  recoveryOperationMatches,
  recoveryPreviewMatches,
} from "./Recovery";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));
vi.mock("react", async (importOriginal) => {
  const actual = await importOriginal<typeof import("react")>();
  return { ...actual, useState: vi.fn(actual.useState) };
});

const entry: RecoveryEntry = {
  id: 8,
  job_id: 3,
  plan_version: 2,
  category: "retained",
  location_id: 1,
  folder: ".domus-retained-generated",
  recorded_bytes: 42,
  version: "original-version",
  state: "retained",
  eligible: true,
  last_error: null,
  operation_id: null,
};
const preview: RecoveryPreview = {
  entry,
  files: [{ name: "original.mp3", bytes: 42, sha256: "a".repeat(64), identity: "file-id" }],
  root_identity: "root-id",
  directory_identity: "directory-id",
  current_sha256: "b".repeat(64),
  preview_sha256: "c".repeat(64),
  verified: true,
  unavailable_reason: null,
};
const operation: RecoveryOperation = {
  id: 12,
  entry_id: 8,
  job_id: 3,
  plan_version: 2,
  kind: "delete",
  state: "failed",
  removed_files: 1,
  total_files: 2,
  last_error: "Destination offline",
};
const command = {
  busy: false,
  error: undefined,
  run: async (action: () => Promise<void>) => action(),
};

function resource(data: unknown, error?: Error, loading = false) {
  vi.mocked(useResource).mockReturnValue({ data, error, loading, refresh: vi.fn() });
}

function review(
  selected = entry,
  evidence: RecoveryPreview = { ...preview, entry: selected },
  savedOperation: RecoveryOperation | null = null,
  busy = false,
) {
  return renderToStaticMarkup(
    <RecoveryReview
      entry={selected}
      preview={evidence}
      operation={savedOperation}
      busy={busy}
      onDelete={() => {}}
      onContinue={() => {}}
    />,
  );
}

beforeEach(() => {
  vi.mocked(useResource).mockReset();
  vi.mocked(useState).mockClear();
});
afterEach(() => vi.unstubAllGlobals());

describe("Recovery inventory", () => {
  it("uses a bounded all-job cursor and request-bound links", async () => {
    resource([entry]);
    const markup = renderToStaticMarkup(<Recovery />);
    expect(markup).toContain("Recovery inventory");
    expect(markup).toContain("Entry 8");
    expect(markup).toContain("original-version");
    expect(markup).toContain("42 recorded bytes");
    expect(markup).toContain("not a measurement of current disk allocation");
    expect(markup).toContain('href="#/request/3/2"');
    expect(markup).toContain("Inspect entry 8");
    expect(markup).not.toContain("Permanently delete");
    expect(markup).not.toContain("Delete all");
    const fetch = vi.fn().mockResolvedValue(Response.json([entry]));
    vi.stubGlobal("fetch", fetch);
    await vi.mocked(useResource).mock.calls[0]![0](new AbortController().signal);
    expect(fetch.mock.calls[0]?.[0]).toBe("/api/recovery?after=0&limit=50");
  });

  it("adds a job filter without changing the saved plan link", async () => {
    resource([entry]);
    const markup = renderToStaticMarkup(<Recovery job={3} />);
    expect(markup).toContain('href="#/recovery"');
    expect(markup).toContain('href="#/request/3/2"');
    const fetch = vi.fn().mockResolvedValue(Response.json([entry]));
    vi.stubGlobal("fetch", fetch);
    await vi.mocked(useResource).mock.calls[0]![0](new AbortController().signal);
    expect(fetch.mock.calls[0]?.[0]).toBe("/api/recovery?after=0&limit=50&job=3");
  });

  it("enables the next page only for a full bounded page", () => {
    resource(Array.from({ length: 50 }, (_, index) => ({ ...entry, id: index + 1 })));
    const full = renderToStaticMarkup(<Recovery />);
    expect(full).toMatch(/<button(?![^>]*disabled)[^>]*>Next page/);
    resource([entry]);
    expect(renderToStaticMarkup(<Recovery />)).toMatch(/<button[^>]*disabled[^>]*>Next page/);
  });

  it.each([
    [undefined, true],
    [new Error("Inventory offline"), false],
  ])("hides retained rows on loading or a failed refresh (%s)", (error, loading) => {
    resource([entry], error, loading);
    const markup = renderToStaticMarkup(<Recovery />);
    expect(markup).not.toContain("original-version");
    expect(markup).not.toContain("Inspect entry 8");
    expect(markup).not.toContain("No recovery records");
    expect(markup).toContain(error ? "Inventory offline" : "Loading recovery records");
  });

  it("distinguishes empty pages, unknown bytes and ineligible entries", () => {
    resource([]);
    expect(renderToStaticMarkup(<Recovery />)).toContain("No recovery records on this page");
    resource([{ ...entry, recorded_bytes: null, eligible: false, state: "missing" }]);
    const markup = renderToStaticMarkup(<Recovery />);
    expect(markup).toContain("Recorded bytes unknown");
    expect(markup).toContain("Ineligible for new deletion");
    expect(markup).toContain("missing");
  });

  it.each([
    ["/recovery", undefined],
    ["/recovery?job=3", 3],
    ["/recovery?job=0", undefined],
    ["/recovery?job=-1", undefined],
    ["/recovery?job=not-a-job", undefined],
    ["/recovery?job=9007199254740992", undefined],
  ])("parses a safe hash-route job filter: %s", (route, job) => {
    expect(recoveryJobFilter(route)).toBe(job);
  });
});

describe("selected Recovery preview", () => {
  it("reads only the selected entry preview and operation, without a command", async () => {
    resource({ preview, operation: null });
    const markup = renderToStaticMarkup(
      <RecoveryDetail entry={entry} command={command} onChanged={() => {}} />,
    );
    expect(markup).toContain("original.mp3");
    expect(markup).toContain("root-id");
    expect(markup).toContain("directory-id");
    expect(markup).toContain(preview.current_sha256);
    expect(markup).toContain(preview.preview_sha256);
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(Response.json(preview))
      .mockResolvedValueOnce(Response.json(null));
    vi.stubGlobal("fetch", fetch);
    const result = await vi.mocked(useResource).mock.calls[0]![0](new AbortController().signal);
    expect(result).toEqual({ preview, operation: null });
    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/recovery/8/preview",
      "/api/recovery/8/operation",
    ]);
    expect(fetch.mock.calls.every(([, options]) => options.method === "GET")).toBe(true);
    expect(vi.mocked(useResource).mock.calls[0]?.[1]).toContain("8:original-version");
  });

  it.each([
    [undefined, true],
    [new Error("Preview refresh failed"), false],
  ])("hides old preview files and commands during a stale read (%s)", (error, loading) => {
    resource({ preview, operation: null }, error, loading);
    const markup = renderToStaticMarkup(
      <RecoveryDetail entry={entry} command={command} onChanged={() => {}} />,
    );
    expect(markup).not.toContain("original.mp3");
    expect(markup).not.toContain(preview.preview_sha256);
    expect(markup).not.toContain("Review irreversible deletion");
  });

  it("requires acknowledgement before the separate final confirmation", () => {
    const markup = review();
    expect(markup).toContain("You cannot restore this version after deletion");
    expect(markup).toContain("Current album files are not selected");
    expect(markup).toContain('type="checkbox"');
    expect(markup).toMatch(/<button[^>]*disabled[^>]*>Review irreversible deletion/);
    expect(markup).not.toContain("Permanently delete entry");
    expect(markup).not.toContain("Confirm irreversible deletion");
  });

  it("shows a single final irreversible action only after acknowledgement and review", () => {
    vi.mocked(useState).mockReturnValueOnce([true, vi.fn()]).mockReturnValueOnce([false, vi.fn()]);
    const acknowledged = review();
    expect(acknowledged).toMatch(/<button(?![^>]*disabled)[^>]*>Review irreversible deletion/);
    expect(acknowledged).not.toContain("Permanently delete entry");
    vi.mocked(useState).mockReturnValueOnce([true, vi.fn()]).mockReturnValueOnce([true, vi.fn()]);
    const confirming = review();
    expect(confirming).toContain('aria-label="Confirm irreversible deletion"');
    expect(confirming.match(/Permanently delete entry 8/g)).toHaveLength(1);
    expect(confirming).toContain("Keep this version");
    expect(confirming).not.toContain("Review irreversible deletion");
  });

  it.each(["missing", "offline", "changed", "ineligible"])(
    "shows %s without exposing saved file lists as live evidence",
    (state) => {
      const selected = { ...entry, state, eligible: false };
      const markup = review(selected, {
        ...preview,
        entry: selected,
        verified: false,
        preview_sha256: null,
        unavailable_reason: state,
      });
      expect(markup).toContain(`Preview unavailable: ${state}`);
      expect(markup).toContain("ineligible");
      expect(markup).not.toContain("original.mp3");
      expect(markup).not.toContain("Review irreversible deletion");
    },
  );

  it("rejects mismatched selected resources before showing their evidence", () => {
    const markup = review(entry, { ...preview, entry: { ...entry, version: "another-version" } });
    expect(markup).toContain("selected version changed");
    expect(markup).not.toContain("original.mp3");
    expect(markup).not.toContain("Review irreversible deletion");
  });

  it.each([
    { id: 9 },
    { job_id: 4 },
    { plan_version: 3 },
    { version: "another-version" },
    { category: "scratch" },
    { location_id: 2 },
    { folder: "different-folder" },
    { eligible: false },
    { state: "deleted" },
  ])("binds deletion to the exact retained entry: %j", (change) => {
    expect(recoveryPreviewMatches(entry, preview)).toBe(true);
    expect(recoveryPreviewMatches(entry, { ...preview, entry: { ...entry, ...change } })).toBe(
      false,
    );
  });

  it("never deletes without a verified digest or while another command is pending", () => {
    expect(recoveryPreviewMatches(entry, { ...preview, verified: false })).toBe(false);
    expect(recoveryPreviewMatches(entry, { ...preview, preview_sha256: null })).toBe(false);
    expect(review(entry, preview, null, true)).toMatch(/<input[^>]*disabled/);
  });
});

describe("Recovery operation status", () => {
  it.each([null, "Manifest preparation failed"])(
    "explicitly retries admitted scratch manifest preparation with operation ID zero (%s)",
    async (lastError) => {
      const scratch = {
        ...entry,
        category: "scratch",
        state: "pending",
        operation_id: null,
        last_error: lastError,
      };
      const markup = review(scratch);
      expect(markup).toContain("Cleanup was admitted");
      expect(markup).toContain("Retry scratch cleanup");
      expect(markup).not.toContain("Review irreversible deletion");
      for (const state of ["retained", "incomplete"]) {
        expect(review({ ...scratch, state })).not.toContain("Retry scratch cleanup");
      }
      expect(review({ ...scratch, state: "complete" })).not.toContain("Retry scratch cleanup");
      const fetch = vi.fn().mockResolvedValue(Response.json(operation));
      vi.stubGlobal("fetch", fetch);
      await continueRecoveryEntry(scratch, null);
      expect(fetch).toHaveBeenCalledTimes(1);
      expect(fetch.mock.calls[0]?.[0]).toBe("/api/recovery/8/cleanup");
      expect(JSON.parse(fetch.mock.calls[0]?.[1].body)).toEqual({ expected_operation_id: 0 });
      await expect(
        continueRecoveryEntry({ ...scratch, state: "incomplete" }, null),
      ).rejects.toThrow("Refresh the saved operation");
      expect(fetch).toHaveBeenCalledTimes(1);
    },
  );

  it("keeps protected inputs view-only and reports only recorded bytes", () => {
    const input = { ...entry, category: "input", operation_id: 12 };
    const markup = review(input, { ...preview, entry: input }, operation);
    expect(markup).toContain("Protected input data is view-only");
    expect(markup).toContain("42 recorded bytes");
    expect(markup).not.toContain("original.mp3");
    expect(markup).not.toContain("Continue approved deletion");
    expect(markup).not.toContain("Review irreversible deletion");
    expect(markup).not.toContain("Retry scratch cleanup");
  });

  it("shows failed scratch cleanup separately and offers only its existing retry", () => {
    const scratch = { ...entry, category: "scratch", operation_id: 12, eligible: false };
    const markup = review(
      scratch,
      { ...preview, entry: scratch },
      { ...operation, kind: "cleanup" },
    );
    expect(markup).toContain("Scratch cleanup status");
    expect(markup).toContain("separate from album completion");
    expect(markup).toContain("Destination offline");
    expect(markup).toContain("1 of 2 files removed");
    expect(markup).toContain("Retry scratch cleanup");
    expect(markup).not.toContain("Review irreversible deletion");
    expect(markup).not.toContain("Continue approved deletion");
  });

  it("does not start scratch cleanup or mutate pending publication data", () => {
    const scratch = { ...entry, category: "scratch", operation_id: null };
    expect(review(scratch)).not.toContain("Retry scratch cleanup");
    expect(review(scratch)).not.toContain("Review irreversible deletion");
    const pending = { ...entry, category: "pending", operation_id: 12 };
    const markup = review(pending, { ...preview, entry: pending }, operation);
    expect(markup).toContain("Pending publication data is view-only");
    expect(markup).not.toContain("Retry scratch cleanup");
    expect(markup).not.toContain("Continue approved deletion");
    expect(markup).not.toContain("Review irreversible deletion");
  });

  it("continues an approved deletion without requesting a new approval", () => {
    const selected = { ...entry, operation_id: 12 };
    const markup = review(selected, { ...preview, entry: selected }, operation);
    expect(markup).toContain("Continue approved deletion");
    expect(markup).not.toContain("Review irreversible deletion");
    expect(markup).toContain("No operation retries automatically");
    expect(review(selected, { ...preview, entry: selected }, operation, true)).toMatch(
      /<button[^>]*disabled[^>]*>Continue approved deletion/,
    );
  });

  it("does not retry completed or mismatched operations", () => {
    const selected = { ...entry, category: "scratch", operation_id: 12 };
    expect(
      review(selected, { ...preview, entry: selected }, { ...operation, state: "complete" }),
    ).not.toContain("Retry scratch cleanup");
    for (const change of [{ id: 13 }, { entry_id: 9 }, { job_id: 4 }, { plan_version: 3 }]) {
      expect(recoveryOperationMatches(selected, { ...operation, ...change })).toBe(false);
      const markup = review(selected, { ...preview, entry: selected }, { ...operation, ...change });
      expect(markup).toContain("saved operation does not match");
      expect(markup).not.toContain("Retry scratch cleanup");
    }
  });
});

describe("guarded Recovery commands", () => {
  it("posts one exact preview digest with irreversible acknowledgement", async () => {
    const fetch = vi.fn().mockResolvedValue(Response.json(operation));
    vi.stubGlobal("fetch", fetch);
    expect(await deleteRecoveryEntry(entry, preview)).toEqual(operation);
    expect(fetch).toHaveBeenCalledTimes(1);
    const [path, options] = fetch.mock.calls[0]!;
    expect(path).toBe("/api/recovery/8/delete");
    expect(options.method).toBe("POST");
    expect(options.credentials).toBe("same-origin");
    expect(new Headers(options.headers).get("x-domus-csrf")).toBe("1");
    expect(JSON.parse(options.body)).toEqual({
      expected_preview_sha256: preview.preview_sha256,
      acknowledge_irreversible_deletion: true,
    });
  });

  it.each([
    ["scratch", "cleanup"],
    ["retained", "continue"],
  ])("explicitly resumes %s by its exact operation ID", async (category, endpoint) => {
    const fetch = vi.fn().mockResolvedValue(Response.json(operation));
    vi.stubGlobal("fetch", fetch);
    await continueRecoveryEntry({ ...entry, category, operation_id: 12 }, operation);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(fetch.mock.calls[0]?.[0]).toBe(`/api/recovery/8/${endpoint}`);
    expect(JSON.parse(fetch.mock.calls[0]?.[1].body)).toEqual({ expected_operation_id: 12 });
  });

  it("rejects unverified previews, wrong versions and unbound operations before POST", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    await expect(deleteRecoveryEntry(entry, { ...preview, verified: false })).rejects.toThrow(
      "fresh verified preview",
    );
    await expect(deleteRecoveryEntry({ ...entry, operation_id: 12 }, preview)).rejects.toThrow(
      "fresh verified preview",
    );
    await expect(deleteRecoveryEntry({ ...entry, version: "other" }, preview)).rejects.toThrow();
    await expect(continueRecoveryEntry(entry, operation)).rejects.toThrow();
    await expect(
      continueRecoveryEntry({ ...entry, operation_id: 12, category: "pending" }, operation),
    ).rejects.toThrow();
    await expect(
      continueRecoveryEntry({ ...entry, operation_id: 12 }, { ...operation, state: "complete" }),
    ).rejects.toThrow();
    expect(fetch).not.toHaveBeenCalled();
  });

  it.each([401, 409, 500])(
    "never retries a rejected or uncertain deletion (%s)",
    async (status) => {
      const fetch = vi
        .fn()
        .mockResolvedValue(Response.json({ error: "Deletion did not finish" }, { status }));
      vi.stubGlobal("fetch", fetch);
      await expect(deleteRecoveryEntry(entry, preview)).rejects.toMatchObject({ status });
      expect(fetch).toHaveBeenCalledTimes(1);
    },
  );

  it("never retries a lost scratch cleanup response", async () => {
    const fetch = vi.fn().mockRejectedValue(new Error("Connection lost"));
    vi.stubGlobal("fetch", fetch);
    await expect(
      continueRecoveryEntry({ ...entry, category: "scratch", operation_id: 12 }, operation),
    ).rejects.toThrow("Connection lost");
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
