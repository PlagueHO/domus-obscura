import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ApiError } from "../api";
import { Alert, Button, Cover } from "./ui";
import { ArtworkReview } from "../ArtworkReview";
import { OutputPreview } from "../../src/OutputPreview";
import type { ArtworkCandidate } from "../../src/generated/ArtworkCandidate";

describe("production components", () => {
  it("renders cover thumbnails and a separate exact preview below its label", () => {
    const candidate: ArtworkCandidate = {
      id: 4,
      job_id: 1,
      plan_version: 2,
      source: "local",
      input_sha256: "input",
      background: null,
      profile: 1,
      sha256: "exact-jpeg",
      bytes: 500,
      width: 100,
      height: 100,
      approved_at: null,
    };
    const markup = renderToStaticMarkup(
      <ArtworkReview
        base="/api/jobs/1/plans/2"
        candidates={[candidate]}
        selected={candidate}
        onSelect={() => {}}
        refresh={() => {}}
      />,
    );
    expect(markup).toContain('type="radio"');
    expect(markup).toContain('alt="Cover candidate 4"');
    expect(markup).toContain('src="/api/jobs/1/plans/2/artwork/4/image"');
    expect(markup.indexOf("Selected exact JPEG preview")).toBeLessThan(
      markup.indexOf('class="cover-preview"'),
    );
    expect(markup).toContain("JPEG SHA-256");
    expect(markup).toMatch(/disabled[^>]*>Approve this exact cover/);
    expect(markup).not.toContain('style="');
  });
  it("provides production login only for an actual 401", () => {
    const unauthorized = renderToStaticMarkup(
      <Alert error={new ApiError(401, "Sign in required.")} />,
    );
    expect(unauthorized).toContain('href="/auth/login"');
    expect(unauthorized).not.toContain("_anonymous");
    const conflict = renderToStaticMarkup(
      <Alert error={new ApiError(409, "<script>stale plan</script>")} />,
    );
    expect(conflict).toContain("&lt;script&gt;stale plan&lt;/script&gt;");
    expect(conflict).not.toContain('href="/auth/login"');
    expect(conflict).toContain('role="alert"');
  });
  it.each([1, 120])("shows the service wait of %i seconds without setup advice", (seconds) => {
    const markup = renderToStaticMarkup(
      <Alert error={new ApiError(503, "MusicBrainz is rate limited.", seconds)} />,
    );
    expect(markup).toContain(`at least ${seconds} ${seconds === 1 ? "second" : "seconds"}`);
    expect(markup).toContain("It will not retry automatically");
    expect(markup).not.toContain("Check the service or setup");
  });
  it("does not present an unsupported provider pause as a timed retry", () => {
    const markup = renderToStaticMarkup(
      <Alert error={new ApiError(503, "Provider requests are paused.", 4294967295)} />,
    );
    expect(markup).toContain("Review the service before resuming");
    expect(markup).not.toContain("4294967295");
    expect(markup).not.toContain("After that wait");
  });
  it("does not invent timing for service errors without Retry-After", () => {
    const markup = renderToStaticMarkup(
      <Alert error={new ApiError(503, "Service unavailable.")} />,
    );
    expect(markup).toContain("Try the action later");
    expect(markup).not.toContain("Check the service or setup");
    expect(markup).not.toContain("at least");
  });
  it("keeps default button semantics and disabled states", () => {
    const markup = renderToStaticMarkup(<Button disabled>Start Download</Button>);
    expect(markup).toContain('type="button"');
    expect(markup).toContain('data-slot="button"');
    expect(markup).toContain("disabled");
    expect(markup).not.toContain('style="');
  });
  it("renders honest absent artwork without external requests", () => {
    const markup = renderToStaticMarkup(<Cover title="<Album>" />);
    expect(markup).toContain("No artwork loaded");
    expect(markup).not.toContain("<img");
    expect(markup).not.toContain("http");
  });
  it("keeps cover upload locked until permission and exact approval separate", () => {
    const markup = renderToStaticMarkup(
      <ArtworkReview
        base="/api/jobs/1/plans/2"
        candidates={[]}
        selected={undefined}
        onSelect={() => {}}
        refresh={() => {}}
      />,
    );
    expect(markup).toContain("I have permission to use");
    expect(markup).toMatch(/type="file"[^>]*disabled/);
    expect(markup).toContain("No cover candidates yet");
    expect(markup).not.toContain("CAA approved");
  });
  it("displays exact Rust-owned filenames and tag text without template execution", () => {
    const markup = renderToStaticMarkup(
      <OutputPreview
        preview={{
          relative_folder: "A/ARTIST/ALBUM",
          filename: "Artist - Album - 01 - Title & Name.mp3",
          changes: ["A/B -> A_B"],
        }}
      />,
    );
    expect(markup).toContain("A/ARTIST/ALBUM");
    expect(markup).toContain("01 - Title &amp; Name.mp3");
    expect(markup).toContain("A/B -&gt; A_B");
    expect(markup).not.toContain('style="');
  });
});
