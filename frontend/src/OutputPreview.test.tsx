import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { OutputPreview } from "./OutputPreview";

describe("OutputPreview", () => {
  it("shows server-owned names without changing them", () => {
    const markup = renderToStaticMarkup(
      <OutputPreview
        preview={{
          relative_folder: "C/THE CURE/DISINTEGRATION",
          filename: "The Cure - Disintegration - 01 - Plainsong.mp3",
          changes: [],
        }}
      />,
    );
    expect(markup).toContain("C/THE CURE/DISINTEGRATION");
    expect(markup).toContain("01 - Plainsong.mp3");
    expect(markup).toContain("No filename replacements.");
  });

  it("escapes untrusted names and shows the replacement explanation", () => {
    const markup = renderToStaticMarkup(
      <OutputPreview
        preview={{
          relative_folder: "A/ARTIST/ALBUM",
          filename: "<script>alert(1)</script>.mp3",
          changes: ["A/B -> A_B"],
        }}
      />,
    );
    expect(markup).not.toContain("<script>");
    expect(markup).toContain("&lt;script&gt;");
    expect(markup).toContain("A/B -&gt; A_B");
  });
});
