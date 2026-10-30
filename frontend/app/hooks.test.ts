import { useRef, useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useResource, useMetadataResource } from "./hooks";

vi.mock("react", () => ({
  useRef: vi.fn(),
  useState: vi.fn(),
  useEffect: vi.fn(),
  useCallback: <T>(callback: T) => callback,
}));

function snapshot(
  previousKey: string,
  key: string,
  error?: Error,
  loading = false,
  metadata = false,
) {
  const load = async () => ["new result"];
  vi.mocked(useRef).mockReturnValueOnce({ current: load });
  vi.mocked(useRef).mockReturnValueOnce({ current: previousKey });
  for (const value of [["saved result"], error, loading, 0]) {
    vi.mocked(useState).mockReturnValueOnce([value, vi.fn()]);
  }
  return (metadata ? useMetadataResource : useResource)(load, key);
}

beforeEach(() => {
  vi.mocked(useRef).mockReset();
  vi.mocked(useState).mockReset();
});

describe("resource identity", () => {
  it("does not present retained metadata as fresh after a failed refresh", () => {
    expect(
      snapshot("same-release", "same-release", new Error("Provider backoff"), false, true).data,
    ).toBeUndefined();
  });
  it("hides the previous resource before the new key's effect runs", () => {
    const value = snapshot("root:1:Album", "root:2:", new Error("Old root error"));
    expect(value.data).toBeUndefined();
    expect(value.error).toBeUndefined();
    expect(value.loading).toBe(true);
  });

  it("retains same-key data with explicit refresh errors", () => {
    const error = new Error("Refresh failed");
    const value = snapshot("same-folder", "same-folder", error);
    expect(value.data).toEqual(["saved result"]);
    expect(value.error).toBe(error);
    expect(value.loading).toBe(false);
  });

  it("keeps a pending same-key refresh blocked", () => {
    const value = snapshot("same-folder", "same-folder", undefined, true);
    expect(value.data).toEqual(["saved result"]);
    expect(value.loading).toBe(true);
  });
});
