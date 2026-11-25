import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { TagReview } from "../src/generated/TagReview";
import type { TagReviewTrack } from "../src/generated/TagReviewTrack";
import { useCommand, useMetadataResource } from "./hooks";
import {
  defaultTagPolicy,
  FrozenTagPolicy,
  needsTagReview,
  TagPolicyFields,
  TagReviewPanel,
} from "./TagPolicy";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useCommand: vi.fn(),
  useMetadataResource: vi.fn(),
}));

const review: TagReview = {
  policy: defaultTagPolicy(),
  id: 1,
  job_id: 1,
  plan_version: 1,
  question_id: 1,
  sha256: "a".repeat(64),
  approved: false,
  resumed: false,
  tracks: [],
};

function track(trackNumber: number): TagReviewTrack {
  const tags = {
    genre: null,
    composer: null,
    comment: null,
    copyright: null,
    comment_language: null,
  };
  return {
    track_number: trackNumber,
    transfer_id: trackNumber,
    input_bytes: 1,
    input_sha256: "b".repeat(64),
    source: tags,
    output: tags,
  };
}

describe("shared tag policy", () => {
  it("requires the final album page before tag approval or continuation", () => {
    vi.mocked(useCommand).mockReturnValue({ busy: false, error: undefined, run: vi.fn() });
    for (const approved of [false, true]) {
      vi.mocked(useMetadataResource).mockReturnValue({
        data: {
          ...review,
          approved,
          tracks: Array.from({ length: 50 }, (_, index) => track(index + 1)),
        },
        loading: false,
        error: undefined,
        refresh: vi.fn(),
      });
      const fullPage = renderToStaticMarkup(
        <TagReviewPanel job={1} version={1} onChanged={() => {}} />,
      );
      const action = approved ? "Apply saved tag approval" : "Approve exact tags for this album";
      expect(fullPage).toMatch(new RegExp(`<button[^>]*disabled[^>]*>${action}</button>`));
      expect(fullPage).toMatch(/<button[^>]*>Next page<\/button>/);

      vi.mocked(useMetadataResource).mockReturnValue({
        data: { ...review, approved, tracks: [track(1)] },
        loading: false,
        error: undefined,
        refresh: vi.fn(),
      });
      const finalPage = renderToStaticMarkup(
        <TagReviewPanel job={1} version={1} onChanged={() => {}} />,
      );
      expect(finalPage).toMatch(new RegExp(`<button[^>]*>${action}</button>`));
      expect(finalPage).toContain("All tracks are available for this album review.");
    }
  });

  it("keeps automatic removal separate from exact retained-tag review", () => {
    const policy = defaultTagPolicy();
    expect(needsTagReview(policy)).toBe(false);
    policy.genre = { action: "retain" };
    expect(needsTagReview(policy)).toBe(true);
    const markup = renderToStaticMarkup(
      <TagPolicyFields policy={policy} disabled={false} onChange={vi.fn()} />,
    );
    expect(markup).toContain("Genre mode: Review required");
    expect(markup).not.toContain("Genre mode<select");
    expect(markup).toContain("Comment mode");
    expect(markup).toContain("Automatic under the approved plan");
    expect(markup).toContain("Release tags and approved artwork stay locked");
  });

  it("shows frozen updates as escaped text and requires review only when selected", () => {
    const policy = defaultTagPolicy();
    policy.comment = { action: "update", mode: "automatic", value: "<untrusted>" };
    let markup = renderToStaticMarkup(<FrozenTagPolicy policy={policy} />);
    expect(markup).toContain("&lt;untrusted&gt;");
    expect(markup).toContain("No extra tag approval");
    policy.comment.mode = "review";
    markup = renderToStaticMarkup(<FrozenTagPolicy policy={policy} />);
    expect(markup).toContain("Exact tag approval is required before normalization");
    expect(needsTagReview(policy)).toBe(true);
  });

  it("retains read-only controls and identifies legacy plans", () => {
    const markup = renderToStaticMarkup(
      <TagPolicyFields policy={defaultTagPolicy()} disabled onChange={vi.fn()} />,
    );
    expect(markup).toContain('<fieldset disabled="">');
    expect(renderToStaticMarkup(<FrozenTagPolicy policy={null} />)).toContain("Legacy plan");
  });
});
