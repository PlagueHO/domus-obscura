import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError, errorText, query, request } from "./api";

afterEach(() => vi.unstubAllGlobals());

describe("same-origin client", () => {
  it("posts plan approval with no body or content type", async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetch);
    await request("/api/jobs/1/plans/2/approve", { method: "POST" });
    const options = fetch.mock.calls[0]?.[1] as RequestInit;
    expect(options.method).toBe("POST");
    expect(options.body).toBeUndefined();
    expect(new Headers(options.headers).get("x-domus-csrf")).toBe("1");
    expect(new Headers(options.headers).has("Content-Type")).toBe(false);
  });

  it("does not apply a client timeout to long source operations", async () => {
    const fetch = vi.fn().mockResolvedValue(Response.json({ peers: [] }));
    vi.stubGlobal("fetch", fetch);
    await request("/api/local/source-search", { body: { release_id: "release" } });
    const options = fetch.mock.calls[0]?.[1] as RequestInit;
    expect(options.signal).toBeUndefined();
  });

  it.each([
    [
      "/api/source-candidates/3/approve",
      {
        plan_version: 2,
        review_sha256: "review",
        acknowledge_exact_edition_and_permitted_download: true,
      },
    ],
    ["/api/jobs/1/plans/2/artwork/approve", { candidate_id: 4, sha256: "jpeg" }],
  ])("preserves JSON for source and artwork approval: %s", async (path, body) => {
    const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetch);
    await request(path, { body });
    const options = fetch.mock.calls[0]?.[1] as RequestInit;
    expect(new Headers(options.headers).get("Content-Type")).toBe("application/json");
    expect(JSON.parse(options.body as string)).toEqual(body);
  });
  it("sends credentials but no command headers on reads", async () => {
    const fetch = vi.fn().mockResolvedValue(Response.json({ object_id: "_anonymous" }));
    vi.stubGlobal("fetch", fetch);
    expect(await request("/api/session")).toEqual({ object_id: "_anonymous" });
    const options = fetch.mock.calls[0]?.[1] as RequestInit;
    expect(options.credentials).toBe("same-origin");
    expect(options.redirect).toBe("error");
    expect(new Headers(options.headers).has("x-domus-csrf")).toBe(false);
    expect(options.body).toBeUndefined();
  });

  it("sends the exact CSRF guard and handles an empty 202 response", async () => {
    const fetch = vi.fn().mockResolvedValue(new Response(null, { status: 202 }));
    vi.stubGlobal("fetch", fetch);
    await expect(
      request("/api/jobs/1/plans/2/run", {
        body: { source_candidate: 3, acknowledge_new_only_publication: true },
      }),
    ).resolves.toBeUndefined();
    const options = fetch.mock.calls[0]?.[1] as RequestInit;
    const headers = new Headers(options.headers);
    expect(options.method).toBe("POST");
    expect(headers.get("x-domus-csrf")).toBe("1");
    expect(headers.has("Origin")).toBe(false);
    expect(headers.get("Content-Type")).toBe("application/json");
    expect(JSON.parse(options.body as string).source_candidate).toBe(3);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("keeps raw cover bytes and the same command guard", async () => {
    const fetch = vi.fn().mockResolvedValue(Response.json({ id: 5 }));
    vi.stubGlobal("fetch", fetch);
    const file = new File([new Uint8Array([1, 2, 3])], "cover.png", { type: "image/png" });
    await request("/api/jobs/1/plans/2/artwork/local?acknowledge_permitted_use=true", { file });
    const options = fetch.mock.calls[0]?.[1] as RequestInit;
    expect(options.body).toBe(file);
    expect(new Headers(options.headers).get("Content-Type")).toBe("application/octet-stream");
    expect(new Headers(options.headers).get("x-domus-csrf")).toBe("1");
  });

  it("shows server errors and never retries a rejected command", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValue(Response.json({ error: "The plan changed." }, { status: 409 }));
    vi.stubGlobal("fetch", fetch);
    await expect(request("/api/jobs/1/plans/2/approve", { body: {} })).rejects.toMatchObject({
      status: 409,
      message: "The plan changed.",
    });
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it.each(["1", "120", "4294967295"])(
    "retains the service retry interval %s without retrying",
    async (retryAfter) => {
      const fetch = vi
        .fn()
        .mockResolvedValue(
          Response.json(
            { error: "MusicBrainz is rate limited or temporarily unavailable" },
            { status: 503, headers: { "Retry-After": retryAfter } },
          ),
        );
      vi.stubGlobal("fetch", fetch);
      await expect(request("/api/discovery/artists?query=Ramones")).rejects.toMatchObject({
        status: 503,
        retryAfterSeconds: Number(retryAfter),
      });
      expect(fetch).toHaveBeenCalledTimes(1);
    },
  );

  it.each(["", "-1", "1.5", "later", "4294967296"])(
    "does not invent a retry interval for an unsupported header: %s",
    async (retryAfter) => {
      vi.stubGlobal(
        "fetch",
        vi
          .fn()
          .mockResolvedValue(
            Response.json(
              { error: "Unavailable" },
              { status: 503, headers: { "Retry-After": retryAfter } },
            ),
          ),
      );
      await expect(request("/api/discovery/artists?query=Ramones")).rejects.toMatchObject({
        retryAfterSeconds: null,
      });
    },
  );

  it("preserves unauthorized state instead of selecting anonymous mode", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(Response.json({ error: "Sign in is required." }, { status: 401 })),
    );
    await expect(request("/api/session")).rejects.toBeInstanceOf(ApiError);
    await expect(request("/api/session")).rejects.toMatchObject({ status: 401 });
  });

  it("handles non-JSON errors without losing the HTTP status", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("Bad gateway", { status: 502 })));
    await expect(request("/api/session")).rejects.toMatchObject({
      status: 502,
      message: "Request failed (502).",
    });
    expect(errorText(null)).toBe("The request failed.");
  });

  it.each([
    "https://example.test/api/jobs",
    "//example.test/api/jobs",
    "/auth/login",
    "/api\\jobs",
    "/api/jobs#fragment",
  ])("rejects non-API or ambiguous request paths: %s", async (path) => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    await expect(request(path)).rejects.toThrow("Only same-origin API paths");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("encodes provider text as query values", () => {
    const encoded = query({ query: "Artist & Album / ?", limit: 24 });
    expect(new URLSearchParams(encoded).get("query")).toBe("Artist & Album / ?");
    expect(new URLSearchParams(encoded).get("limit")).toBe("24");
  });
});
