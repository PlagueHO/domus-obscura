import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import { Settings } from "./Catalog";

const settings: SettingsVersion = {
  version: 7,
  settings: {
    format: "mp3",
    accepted_formats: ["mp3", "flac"],
    artwork_size: "recommended",
    template: { folder: "<Album Artist>\\<Album>", filename: "<Title>.<ext>" },
  },
};

function render(props: Partial<React.ComponentProps<typeof Settings>> = {}) {
  return renderToStaticMarkup(
    <Settings
      settings={settings}
      locations={[]}
      policy={{ read_only: true }}
      onSaved={() => {}}
      onReload={() => {}}
      {...props}
    />,
  );
}

describe("system library settings", () => {
  it.each([true, undefined])("shows saved values without enabling writes (%s)", (readOnly) => {
    const markup = render({
      policy: readOnly === undefined ? undefined : { read_only: readOnly },
    });
    expect(markup).toContain("This form is read-only.");
    expect(markup).toContain("MP3");
    expect(markup).toContain("400 × 400 px");
    expect(markup).toContain("&lt;Album Artist&gt;");
    expect(markup).toContain("&lt;Title&gt;.&lt;ext&gt;");
    expect(markup).not.toContain("<form");
    expect(markup).not.toContain("Save system output settings");
  });

  it.each([
    ["small", "200 × 200 px"],
    ["large", "800 × 800 px"],
    [null, "Legacy · up to 1200 × 1200 px"],
  ] as const)("shows the saved artwork target %s", (artworkSize, label) => {
    const markup = render({
      settings: { ...settings, settings: { ...settings.settings, artwork_size: artworkSize } },
    });
    expect(markup).toContain(label);
  });

  it("enables typed editing only with explicit permission", () => {
    const markup = render({ policy: { read_only: false } });
    expect(markup).toContain("<form");
    expect(markup).toContain("Save system output settings");
    expect(markup).toContain("The administrator controls these settings for the shared library.");
    expect(markup).toContain("apply to all users, not to individual accounts.");
    expect(markup).toContain("400 × 400 px · Recommended");
    expect(markup).not.toContain("Legacy ·");
    expect(markup).toContain('value="&lt;Title&gt;.&lt;ext&gt;"');
    expect(markup).toContain("Existing plans keep their frozen settings.");
  });

  it("shows separate accepted and preferred combinations and root configuration", () => {
    const markup = render({ policy: { read_only: false } });
    expect(markup).toContain("Accepted audio formats and bitrate");
    expect(markup).toContain("Preferred audio format and bitrate");
    expect(markup.match(/type="checkbox"/g)).toHaveLength(2);
    expect(markup.match(/type="checkbox"[^>]*checked/g)).toHaveLength(2);
    expect(markup).toContain("320 kbps CBR");
    expect(markup).toContain("Lossless (no fixed bitrate)");
    expect(markup).toContain("Other MP3 bitrates, VBR and M4A are not supported yet.");
    expect(markup).toContain("Register library root");
    expect(markup).toContain("Server folder path");
  });

  it("requires a preferred choice within the accepted combinations", () => {
    const markup = render({
      policy: { read_only: false },
      settings: { ...settings, settings: { ...settings.settings, accepted_formats: ["flac"] } },
    });
    expect(markup).toContain("Choose an accepted combination");
    expect(markup).toContain("Select at least one accepted combination");
    expect(markup).toMatch(/<button[^>]*type="submit"[^>]*disabled/);
  });

  it("shows legacy accepted choices and hides all root writes in read-only mode", () => {
    const markup = render({
      settings: { ...settings, settings: { ...settings.settings, accepted_formats: null } },
    });
    expect(markup).toContain("MP3");
    expect(markup).toContain("Lossless");
    expect(markup).not.toContain("Register library root");
    expect(markup).not.toContain("<form");
  });

  it("shows read failures instead of reporting missing setup", () => {
    const markup = render({
      settings: undefined,
      policy: undefined,
      error: new Error("Setup is unavailable."),
    });
    expect(markup).toContain('role="alert"');
    expect(markup).toContain("Setup is unavailable.");
    expect(markup).toContain("Reload system settings");
    expect(markup).not.toContain("Run local setup.");
  });

  it("disables cached editors during a failed or pending refresh", () => {
    for (const props of [{ loading: true }, { error: new Error("Refresh failed.") }]) {
      const markup = render({ policy: { read_only: false }, ...props });
      expect(markup).toMatch(/<button[^>]*type="submit"[^>]*disabled/);
      expect(markup).toMatch(/<input[^>]*disabled/);
    }
  });

  it("keeps loading distinct from missing configuration", () => {
    const markup = render({ settings: undefined, loading: true });
    expect(markup).toContain("Loading system output settings");
    expect(markup).toContain("Loading library roots");
    expect(markup).not.toContain("Run local setup.");
  });
});
