import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { LocalTest } from "./LocalTest";
import { request, sampleEdition } from "./client";

afterEach(() => vi.unstubAllGlobals());

describe("local test harness", () => {
  it("shows its safety boundary and available operations before loading", () => {
    const markup = renderToStaticMarkup(<LocalTest />);
    expect(markup).toContain("Local test harness");
    expect(markup).toContain("Your music library is not connected.");
    expect(markup).toContain("Data resets when the server stops.");
    expect(markup).toContain("Preview output");
    expect(markup).toContain("Run sample album test");
    expect(markup).toContain("not a Soulseek download");
    expect(markup).toContain("Save plan using saved settings");
    expect(markup).toContain("Browse the sample library");
    expect(markup).toContain('role="status"');
    expect(markup).not.toContain("https://");
    expect(sampleEdition.tracks[2]?.disc).toBe(2);
  });

  it("sends commands with CSRF and reports backend conflicts", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify({ error: "plan observations changed" }), { status: 409 }),
      );
    vi.stubGlobal("fetch", fetch);
    await expect(request("/local/approve", "POST", { version: 2 })).rejects.toThrow(
      "plan observations changed",
    );
    expect(fetch).toHaveBeenCalledWith(
      "/local/approve",
      expect.objectContaining({
        method: "POST",
        headers: { "x-domus-csrf": "1", "content-type": "application/json" },
        body: '{"version":2}',
      }),
    );
  });

  it("reports plain-text extractor errors and returns successful JSON", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(new Response("body too large", { status: 413 })),
    );
    await expect(request("/local/preview", "POST", {})).rejects.toThrow("body too large");
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response('{"plan_version":2}')));
    await expect(request("/local/snapshot")).resolves.toEqual({ plan_version: 2 });
  });
});
