import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import { BackendConfiguration } from "./BackendConfiguration";
import { useResource } from "./hooks";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

const configuration: BackendConfigurationState["active"] = {
  environment: "development",
  system_settings_read_only: false,
  services: {
    discovery: ["musicbrainz"],
    metadata: ["musicbrainz"],
    artwork: ["cover-art-archive"],
    acquisition: ["soulseek"],
  },
  cache: { root: "generated-cache", item_bytes: 9437184, total_bytes: 134217728, entries: 64 },
  database: "generated-database",
  listen: "127.0.0.1:8443",
  origin: "https://localhost:8443",
  tenant_id_configured: false,
  object_id_configured: false,
  client_id_configured: false,
  client_secret_file_configured: false,
  certificate_file: "generated-certificate",
  private_key_file_configured: true,
  browser_assets: "generated-browser",
  local_acquisition: {
    staging_root: "generated-staging",
    credentials_file_configured: true,
    server: "8.8.8.8:2242",
    acknowledge_plaintext_login: true,
  },
};

const state: BackendConfigurationState = {
  active: configuration,
  saved: configuration,
  active_revision: "a".repeat(64),
  saved_revision: "a".repeat(64),
  restart_required: false,
  saved_requires_validation: false,
};

function render(data: BackendConfigurationState | undefined, error?: Error, loading = false) {
  vi.mocked(useResource).mockReturnValue({ data, error, loading, refresh: vi.fn() });
  return renderToStaticMarkup(<BackendConfiguration />);
}

beforeEach(() => vi.mocked(useResource).mockReset());

describe("backend configuration inspection", () => {
  it("shows all configured groups without write controls or secret values", () => {
    const markup = render(state);
    expect(markup).toContain("Saved JSON matches the active configuration.");
    expect(markup).toContain("generated-database");
    expect(markup).toContain("generated-browser");
    expect(markup).toContain("generated-cache");
    expect(markup).toContain("generated-staging");
    expect(markup).toContain("cover-art-archive");
    expect(markup).toContain("Entra secret file");
    expect(markup).toContain("TLS private-key file");
    expect(markup).toContain("Configured · value hidden");
    expect(markup).toContain("Library policy can be edited");
    expect(markup).toContain("does not save backend JSON");
    expect(markup).not.toContain("<form");
    expect(markup).not.toContain("<input");
    expect(markup).not.toContain("Save backend");
  });

  it("does not present changed saved JSON as active or validated", () => {
    const markup = render({
      ...state,
      saved: { ...configuration, origin: "https://localhost:9443" },
      saved_revision: "b".repeat(64),
      restart_required: true,
      saved_requires_validation: true,
    });
    expect(markup).toContain("It is not active.");
    expect(markup).toContain("full startup validation");
    expect(markup).toContain("https://localhost:8443");
    expect(markup).not.toContain("https://localhost:9443");
    expect(markup).not.toContain("matches the active");
  });

  it("keeps failures and stale reads distinct from disabled configuration", () => {
    const failed = render(undefined, new Error("Saved JSON is invalid."));
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("Saved JSON is invalid.");
    expect(failed).not.toContain("Legacy SQLite cache");
    expect(failed).not.toContain("matches the active");
    const stale = render(state, new Error("Saved file unavailable."));
    expect(stale).toContain("Last loaded configuration.");
    expect(stale).toContain("could not be checked");
    expect(stale).toContain("generated-database");
    expect(stale).not.toContain("matches the active");
    const reloading = render(state, undefined, true);
    expect(reloading).toContain("Values from the last successful read remain below.");
    expect(reloading).not.toContain("matches the active");
    expect(render(undefined, undefined, true)).toContain("Loading backend configuration");
  });

  it("shows omitted optional services without inventing cache or acquisition state", () => {
    const value = { ...configuration, cache: null, browser_assets: null, local_acquisition: null };
    const markup = render({ ...state, active: value, saved: value });
    expect(markup).toContain("Legacy SQLite cache");
    expect(markup).toContain("Development acquisition");
    expect(markup).not.toContain("Private staging root");
    expect(markup).not.toContain("Maximum cache item");
    expect(markup).toContain("Not configured");
  });
});
