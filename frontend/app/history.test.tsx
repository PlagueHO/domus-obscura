import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { JobEvent } from "../src/generated/JobEvent";
import { useResource } from "./hooks";
import { RequestEvents, RequestHistory } from "./RequestHistory";

vi.mock("./hooks", () => ({ useResource: vi.fn() }));
const event: JobEvent = {
  id: 8,
  job_id: 3,
  plan_version: 2,
  event_type: "plan_approved",
  created_at: 1,
};

function render(data: JobEvent[] | undefined, error?: Error, loading = false) {
  vi.mocked(useResource).mockReturnValue({ data, error, loading, refresh: vi.fn() });
  return renderToStaticMarkup(
    <RequestHistory base="/api/jobs/3/plans/2" revision={0} active={false} />,
  );
}

beforeEach(() => vi.mocked(useResource).mockReset());

describe("saved plan history", () => {
  it("shows exact saved events without inventing success or a retry action", () => {
    const markup = renderToStaticMarkup(
      <RequestEvents events={[event, { ...event, id: 9, event_type: "generated_<event>" }]} />,
    );
    expect(markup).toContain("plan approved");
    expect(markup).toContain("generated &lt;event&gt;");
    expect(markup).toContain("Event 8 · Plan 2");
    expect(markup).toMatch(/datetime="1970-01-01T00:00:01.000Z"/i);
    expect(markup).not.toContain("<button");
  });

  it("keeps history separate from acquisition and unknown legacy plan binding", () => {
    const markup = render([event]);
    expect(markup).toContain("Events without a saved plan version");
    expect(markup).toContain("It does not resume or retry work");
    expect(markup).toContain("Reload history");
    expect(markup).not.toContain("Start Download");
    expect(markup).not.toContain("Approve");
  });

  it("keeps cached rows with explicit errors instead of an empty history claim", () => {
    const failed = render([event], new Error("History unavailable"));
    expect(failed).toContain("History unavailable");
    expect(failed).toContain("Retained rows are not a current response");
    expect(failed).toContain("plan approved");
    expect(failed).not.toContain("No events on this page");
    expect(render([], undefined, true)).not.toContain("No events on this page");
    expect(render([])).toContain("No events on this page");
  });
});
