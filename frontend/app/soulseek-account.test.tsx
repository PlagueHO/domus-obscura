import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, expect, it, vi } from "vitest";
import { SoulseekAccount } from "./SoulseekAccount";
import { useResource } from "./hooks";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(useResource).mockReset();
});

it("uses write-only fields and distinguishes account storage from connection and capabilities", () => {
  vi.mocked(useResource).mockReturnValue({
    data: {
      available: true,
      reason: null,
      configured: false,
      active_revision: "a".repeat(64),
      pending_change_id: null,
    },
    error: undefined,
    loading: false,
    refresh: vi.fn(),
  });
  const markup = renderToStaticMarkup(<SoulseekAccount disabled={false} onChanged={() => {}} />);
  expect(markup).toContain('type="password"');
  expect(markup).toContain('autoComplete="new-password"');
  expect(markup).toContain("The password cannot be read back");
  expect(markup).toContain("Saving does not sign in");
  expect(markup).toContain("plaintext TCP");
  expect(markup).toContain("Save account");
  expect(markup).not.toContain("Restart and apply account");
});

it("blocks entry for read-only or stale account status and does not call it current", () => {
  for (const disabled of [false, true]) {
    vi.mocked(useResource).mockReturnValue({
      data: {
        available: true,
        reason: null,
        configured: true,
        active_revision: "a".repeat(64),
        pending_change_id: null,
      },
      error: disabled ? undefined : new Error("Refresh failed"),
      loading: false,
      refresh: vi.fn(),
    });
    const markup = renderToStaticMarkup(
      <SoulseekAccount disabled={disabled} onChanged={() => {}} />,
    );
    expect(markup).toContain('type="password"');
    expect(markup).toMatch(/type="password"[^>]*disabled/);
    if (!disabled) expect(markup).not.toContain("An account is configured on this host");
  }
});
