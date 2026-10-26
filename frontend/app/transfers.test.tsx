import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TransferSummary } from "../src/generated/TransferSummary";
import { TransferProgress, allInputsReceived } from "./TransferProgress";

const complete: TransferSummary = {
  job_id: 3,
  plan_version: 2,
  expected_tracks: 137,
  total: 137,
  planned: 0,
  submitting: 0,
  active: 0,
  received: 137,
  all_received: true,
};

describe("whole-album saved transfers", () => {
  it("uses the whole plan independently of a receipt page", () => {
    expect(allInputsReceived(complete, 3, 2, 137)).toBe(true);
    expect(
      allInputsReceived(
        { ...complete, expected_tracks: 1000, total: 1000, received: 1000 },
        3,
        2,
        1000,
      ),
    ).toBe(true);
    for (const changed of [
      { job_id: 4 },
      { plan_version: 3 },
      { expected_tracks: 136 },
      { total: 100, received: 100 },
      { received: 136, planned: 1 },
      { received: 136, submitting: 1 },
      { received: 136, active: 1 },
      { all_received: false },
    ]) {
      expect(allInputsReceived({ ...complete, ...changed }, 3, 2, 137)).toBe(false);
    }
    expect(allInputsReceived(undefined, 3, 2, 137)).toBe(false);
    expect(
      allInputsReceived({ ...complete, total: 0, expected_tracks: 0, received: 0 }, 3, 2, 0),
    ).toBe(false);
  });

  it("keeps uncertain submission distinct from receipt and publication", () => {
    const markup = renderToStaticMarkup(
      <TransferProgress
        summary={{ ...complete, received: 136, submitting: 1, all_received: false }}
      />,
    );
    expect(markup).toContain("136 of 137 tracks received");
    expect(markup).toContain("Tracks with uncertain submission: 1");
    expect(markup).toContain("A receipt page cannot authorize a retry");
    expect(markup).toContain("not prove current staging contents or completed publication");
    expect(markup).not.toContain("<button");
  });
});
