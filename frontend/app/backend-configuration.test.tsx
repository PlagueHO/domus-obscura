import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import {
  BackendConfiguration,
  sharingWorkerEvidence,
  ManagedCertificateStatus,
} from "./BackendConfiguration";
import { useResource } from "./hooks";

vi.mock("./hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./hooks")>()),
  useResource: vi.fn(),
}));

const configuration: BackendConfigurationState["active"] = {
  production_worker: null,
  environment: "development",
  system_settings_read_only: false,
  services: {
    discovery: ["musicbrainz"],
    metadata: ["musicbrainz"],
    artwork: ["cover-art-archive"],
    acquisition: ["soulseek"],
    artist_images: [],
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
  tls_mode: "manual",
  managed_tls: null,
  browser_assets: "generated-browser",
  local_acquisition: null,
};
const state: BackendConfigurationState = {
  active: configuration,
  saved: configuration,
  active_revision: "a".repeat(64),
  saved_revision: "a".repeat(64),
  restart_required: false,
  saved_requires_validation: false,
  changed_sections: [],
};
function render(data: BackendConfigurationState | undefined, error?: Error, loading = false) {
  vi.mocked(useResource).mockReturnValue({ data, error, loading, refresh: vi.fn() });
  return renderToStaticMarkup(<BackendConfiguration />);
}
beforeEach(() => {
  vi.mocked(useResource).mockReset();
  vi.unstubAllGlobals();
});

describe("operator-owned host configuration", () => {
  it("shows host settings without save or reload controls under either policy", () => {
    for (const readOnly of [false, true]) {
      const value = { ...configuration, system_settings_read_only: readOnly };
      const markup = render({ ...state, active: value, saved: value });
      expect(markup).toContain("generated-browser");
      expect(markup).toContain("Other host settings are managed by the operator");
      expect(markup).toContain("Configuration sections");
      expect(markup).toContain('aria-label="Refresh settings"');
      expect(markup).toContain('class="design-card-header"');
      expect(markup).not.toContain("<form");
      expect(markup).not.toContain("Save changes");
      expect(markup).not.toContain(">Reload</button>");
    }
  });
  it("keeps saved and active revisions distinct and requires a coordinated restart", () => {
    const markup = render({
      ...state,
      saved: { ...configuration, origin: "https://localhost:9443" },
      saved_revision: "b".repeat(64),
      restart_required: true,
      saved_requires_validation: true,
      changed_sections: ["general"],
    });
    expect(markup).toContain("Saved host settings are not active");
    expect(markup).toContain("coordinated restart is required");
    expect(markup).toContain("https://localhost:9443");
    expect(markup).toContain("https://localhost:8443");
    expect(markup).toContain("before all components restart");
    expect(markup).not.toContain("Reload needed");
  });
  it("labels retained values after refresh failure or while refreshing", () => {
    expect(render(state, new Error("Refresh failed"))).toContain("last loaded values are shown");
    expect(render(state, undefined, true)).toContain("Refreshing settings");
    expect(render(undefined)).toContain("Loading settings");
  });
  it("keeps SQLite output and library controls separate from host ownership", () => {
    vi.mocked(useResource).mockReturnValue({
      data: state,
      error: undefined,
      loading: false,
      refresh: vi.fn(),
    });
    const markup = renderToStaticMarkup(
      <BackendConfiguration
        output={
          <form aria-label="Shared SQLite policy">
            <button>Save output policy</button>
          </form>
        }
        roots={<button>Register library root</button>}
      />,
    );
    expect(markup).toContain("Save output policy");
    expect(markup).toContain("Register library root");
  });
  it("shows production acquisition details without credential paths or editing controls", () => {
    vi.stubGlobal("location", { hash: "#/settings?section=music_acquisition" });
    const value = {
      ...configuration,
      production_worker: {
        queue_capacity: 7,
        sharing: null,
        acquisition: {
          staging_root: "generated-staging",
          credentials_file_configured: true,
          server: "8.8.8.8:2242",
          acknowledge_plaintext_login: true,
        },
      },
    };
    const markup = render({ ...state, active: value, saved: value });
    const [acquisition, account] = markup.split("</section>", 2);
    expect(acquisition).toContain("generated-staging");
    expect(acquisition).toContain("Queue capacity");
    expect(acquisition).toContain("value hidden");
    expect(acquisition).not.toContain("<form");
    expect(account).toContain("<h2>Soulseek account</h2>");
    expect(account).toMatch(/<input[^>]*type="password"[^>]*disabled/);
  });
});

describe("current runtime evidence", () => {
  it("uses the fresh worker heartbeat rather than the API snapshot", () => {
    const worker = {
      online: true,
      stopping: false,
      active_revision: state.saved_revision,
      capacity: 64,
      unfinished: 0,
      active: 0,
    };
    expect(sharingWorkerEvidence(worker, state.saved_revision, true)).toContain(
      "heartbeat confirms",
    );
    expect(sharingWorkerEvidence(worker, "another-revision", true)).toContain(
      "different configuration revision",
    );
    expect(sharingWorkerEvidence(worker, state.saved_revision, false)).toContain("unknown");
    expect(
      sharingWorkerEvidence({ ...worker, online: false }, state.saved_revision, true),
    ).toContain("offline");
    expect(
      sharingWorkerEvidence({ ...worker, stopping: true }, state.saved_revision, true),
    ).toContain("stopping");
    expect(sharingWorkerEvidence(undefined, state.saved_revision, true)).toContain("unknown");
  });
  it("hides certificate dates after a failed refresh or a changed configuration key", () => {
    const certificate = {
      phase: "ready",
      active_generation: 1,
      expires_at: 2000000000,
      renew_at: 1900000000,
      next_attempt: null,
      attempts: 0,
      last_success: 1800000000,
      recovery_required: false,
      cleanup_pending: false,
    };
    for (const resource of [
      { data: { configuration_revision: "other", certificate }, loading: false, error: undefined },
      {
        data: { configuration_revision: "current", certificate },
        loading: false,
        error: new Error("Refresh failed"),
      },
    ]) {
      vi.mocked(useResource).mockReturnValue({ ...resource, refresh: vi.fn() });
      const markup = renderToStaticMarkup(<ManagedCertificateStatus revision="current" />);
      expect(markup).toContain("Certificate status is unknown");
      expect(markup).not.toContain("Certificate expiry");
    }
  });
});
