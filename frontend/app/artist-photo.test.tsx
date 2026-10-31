import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { ArtistPhotoView, PhotoCredit } from "./ArtistPhoto";
import { ArtistPhotoBindings } from "./ArtistPhotoBindings";
import { generatedPhoto } from "../local-test/src/ArtistPhotoTest";

describe("artist photo evidence", () => {
  it("shows complete credit as text with source and license links", () => {
    const markup = renderToStaticMarkup(
      <PhotoCredit
        rights={{
          ...generatedPhoto.photo.source.rights,
          creator: "<script>untrusted</script>",
        }}
      />,
    );
    expect(markup).toContain("&lt;script&gt;untrusted&lt;/script&gt;");
    expect(markup).not.toContain("<script>");
    expect(markup).toContain("https://creativecommons.org/publicdomain/zero/1.0/");
    expect(markup).toContain("Normalized and resized without cropping");
    expect(markup).toContain("Generated locally");
  });
  it("keeps all missing states distinct and does not render a photo", () => {
    for (const status of ["disabled", "unbound", "missing"] as const) {
      const markup = renderToStaticMarkup(
        <ArtistPhotoView title="Fixture" state={{ status }} onRetry={vi.fn()} />,
      );
      expect(markup).not.toContain("<img");
      expect(markup).toContain(
        status === "disabled"
          ? "disabled"
          : status === "missing"
            ? "missing from Commons"
            : "No photo is linked",
      );
    }
  });
  it("hides retained photo and credits during loading or a failed refresh", () => {
    for (const props of [{ loading: true }, { error: new Error("Rights changed") }]) {
      const markup = renderToStaticMarkup(
        <ArtistPhotoView title="Fixture" state={generatedPhoto} {...props} onRetry={vi.fn()} />,
      );
      expect(markup).not.toContain("<img");
      expect(markup).not.toContain("Photo credit");
      expect(markup).toContain("loading" in props ? "Loading artist photo" : "Rights changed");
    }
  });
  it("preserves natural dimensions and keeps credit outside navigation links", () => {
    const markup = renderToStaticMarkup(
      <ArtistPhotoView title="Fixture" state={generatedPhoto} onRetry={vi.fn()} />,
    );
    expect(markup).toContain('width="320" height="160"');
    expect(markup).toContain("<figcaption>");
    expect(markup).toContain("<details");
    expect(markup).not.toMatch(/<a[^>]*>(?:(?!<\/a>)[\s\S])*<a/);
  });
  it("disables binding changes before source activation and on read-only hosts", () => {
    for (const props of [
      { readOnly: false, enabled: false },
      { readOnly: true, enabled: true },
    ]) {
      const markup = renderToStaticMarkup(<ArtistPhotoBindings {...props} />);
      expect(markup).toContain('<fieldset disabled="">');
      expect(markup).toContain("MusicBrainz artist ID");
      expect(markup).toContain("Commons File title");
      expect(markup).not.toContain("Save artist photo link");
    }
  });
});
