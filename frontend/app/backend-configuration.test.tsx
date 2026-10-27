import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import { BackendConfiguration, confirmConfigurationReload } from "./BackendConfiguration";
import { ApiError } from "./api";
import { useResource } from "./hooks";
import { BackendConfigurationForm } from "./BackendConfigurationForm";
import {
  beginConfigurationEdit,
  changeConfigurationDraft,
  configurationErrors,
  acceptConfigurationSave,
  dirtySections,
  sectionUpdate,
  discardConfigurationSection,
} from "./backend-configuration-state";

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
  changed_sections: [],
  reload: { status: "idle" },
};

function render(data: BackendConfigurationState | undefined, error?: Error, loading = false) {
  vi.mocked(useResource).mockReturnValue({ data, error, loading, refresh: vi.fn() });
  return renderToStaticMarkup(<BackendConfiguration />);
}

beforeEach(() => vi.mocked(useResource).mockReset());

describe("backend configuration inspection", () => {
  it("shows section navigation and the selected group without exposing other fields", () => {
    const markup = render(state);
    expect(markup).not.toContain("Configuration state");
    expect(markup).not.toContain("Configuration version");
    expect(markup).toContain("generated-browser");
    expect(markup).toContain("<fieldset><legend>General</legend>");
    expect(markup).toContain('<fieldset disabled="" hidden=""><legend>Providers</legend>');
    expect(markup).toContain("Core application");
    expect(markup).toContain("Media");
    expect(markup).toContain("Music");
    expect(markup).toContain("Identity");
    expect(markup).toContain("TLS");
    expect(markup).toContain("Providers");
    expect(markup).toContain("Acquisition");
    expect(markup).toContain('aria-label="Refresh settings"');
    expect(markup).toContain('class="design-card-header"');
    expect(markup).toContain("Save changes");
    expect(markup).toContain("<form");
    expect(markup).toContain('name="listen"');
    expect(markup).not.toContain(">Edit backend configuration</button>");
    expect(markup).not.toContain("Saved changes need a reload");
    expect(markup).toContain("<summary>Advanced</summary>");
    expect(markup).not.toContain("Save backend");
  });

  it("never offers editing for active or saved read-only policy or stale reads", () => {
    const restricted = { ...configuration, system_settings_read_only: true };
    for (const value of [
      { ...state, active: restricted },
      { ...state, saved: restricted },
    ]) {
      const markup = render(value);
      expect(markup).not.toContain("Edit backend configuration");
      expect(markup).not.toContain("<form");
      expect(markup).toContain("operator must enable editing");
    }
    expect(render(state, new Error("Refresh failed"))).toMatch(
      /<button[^>]*type="submit"[^>]*disabled[^>]*>Save changes/,
    );
  });

  it("does not present changed saved JSON as active or validated", () => {
    const markup = render({
      ...state,
      saved: { ...configuration, origin: "https://localhost:9443" },
      saved_revision: "b".repeat(64),
      restart_required: true,
      saved_requires_validation: true,
    });
    expect(markup).toContain("Saved changes need a reload");
    expect(markup).toContain("Reload the API to apply saved settings");
    expect(markup).toContain("full validation");
    expect(markup).toContain("https://localhost:8443");
    expect(markup).toMatch(/name="origin"[^>]*value="https:\/\/localhost:9443"/);
    expect(markup).toContain("Currently running");
  });

  describe("backend configuration draft", () => {
    it("copies every public field but never copies a hidden value or the operator switch", () => {
      const editor = beginConfigurationEdit(state);
      expect(editor.expectedRevision).toBe(state.saved_revision);
      expect(editor.draft).not.toHaveProperty("system_settings_read_only");
      expect(editor.draft.private_key_file).toEqual({ action: "keep" });
      expect(editor.draft.local_acquisition?.credentials_file).toEqual({ action: "keep" });
      editor.draft.services.discovery.length = 0;
      expect(state.saved.services.discovery).toEqual(["musicbrainz"]);
    });

    it("retains the full draft on errors and invalidates validation after an edit", () => {
      const editor = beginConfigurationEdit(state);
      const changed = changeConfigurationDraft(editor, {
        ...editor.draft,
        private_key_file: { action: "replace", value: "generated-replacement" },
      });
      const failed = configurationErrors(changed, [{ field: "origin", message: "Invalid origin" }]);
      expect(failed.draft).toBe(changed.draft);
      expect(failed.validated).toBe(false);
      expect(failed.expectedRevision).toBe(editor.expectedRevision);
      const validated = configurationErrors(changed, []);
      expect(validated.validated).toBe(true);
      const editedAgain = changeConfigurationDraft(validated, {
        ...validated.draft,
        origin: "https://localhost:9443",
      });
      expect(editedAgain.validated).toBe(false);
      expect(editedAgain.draft.private_key_file).toEqual({
        action: "replace",
        value: "generated-replacement",
      });
    });

    it("renders all typed control groups, validation errors, and restart-only actions", () => {
      const editor = configurationErrors(beginConfigurationEdit(state), [
        { field: "origin", message: "Use HTTPS." },
      ]);
      const markup = renderToStaticMarkup(
        <BackendConfigurationForm
          editor={editor}
          disabled={false}
          busy={false}
          onChange={vi.fn()}
          onValidate={vi.fn()}
          onSave={vi.fn()}
          onDiscard={vi.fn()}
        />,
      );
      for (const label of [
        "Environment",
        "Local database",
        "Listen address",
        "Browser origin",
        "Browser assets",
        "Providers",
        "Cache root",
        "Maximum cache entries",
        "Entra identity",
        "TLS certificate file",
        "TLS private-key file",
        "Private staging root",
        "Soulseek credential file",
        "Soulseek server",
        "Validate backend configuration",
        "Save backend configuration",
        "Discard backend draft",
      ])
        expect(markup).toContain(label);
      expect(markup).toContain("Use HTTPS.");
      expect(markup).toContain('aria-invalid="true"');
      expect(markup).toContain("Saving does not activate settings");
      expect(markup).not.toContain('name="system_settings_read_only"');
      expect(markup).not.toContain('type="password"');
    });
  });

  it("keeps failures and stale reads distinct from disabled configuration", () => {
    const failed = render(undefined, new Error("Saved JSON is invalid."));
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("Saved JSON is invalid.");
    expect(failed).not.toContain("Legacy SQLite cache");
    expect(failed).not.toContain("matches the active");
    const stale = render(state, new Error("Saved file unavailable."));
    expect(stale).toContain("The last loaded values are shown.");
    expect(stale).toContain("could not be checked");
    expect(stale).toContain("generated-browser");
    expect(stale).not.toContain("matches the active");
    const reloading = render(state, undefined, true);
    expect(reloading).toContain("Refreshing settings. The last loaded values are shown.");
    expect(reloading).not.toContain("matches the active");
    expect(render(undefined, undefined, true)).toContain("Loading settings");
  });

  it("shows omitted optional services without inventing cache or acquisition state", () => {
    const value = { ...configuration, cache: null, browser_assets: null, local_acquisition: null };
    const markup = render({ ...state, active: value, saved: value });
    expect(markup).toContain("Storage/cache");
    expect(markup).toContain("Acquisition");
    expect(markup).not.toContain('name="local_acquisition.staging_root"');
    expect(markup).not.toContain('name="cache.item_bytes"');
    expect(markup).toContain('name="browser_assets"');
  });

  describe("section save state", () => {
    it("sends only owned fields and retains other section drafts after save", () => {
      const initial = beginConfigurationEdit(state);
      const editor = changeConfigurationDraft(initial, {
        ...initial.draft,
        origin: "https://localhost:9443",
        listen: "127.0.0.1:9443",
        private_key_file: { action: "replace", value: "retained-hidden-reference" },
      });
      expect(dirtySections(editor)).toEqual(["general", "tls"]);
      const update = sectionUpdate(editor.draft, "general");
      expect(update).not.toHaveProperty("private_key_file");
      expect(update).not.toHaveProperty("services");
      const saved = {
        ...state,
        saved_revision: "b".repeat(64),
        saved: { ...configuration, origin: editor.draft.origin, listen: editor.draft.listen },
      };
      const next = acceptConfigurationSave(editor, saved, ["general"]);
      expect(next.expectedRevision).toBe(saved.saved_revision);
      expect(dirtySections(next)).toEqual(["tls"]);
      expect(next.draft.private_key_file).toEqual(editor.draft.private_key_file);
      expect(next.draft.origin).toBe(editor.draft.origin);
      const final = acceptConfigurationSave(next, saved, ["tls"]);
      expect(dirtySections(final)).toEqual([]);
      expect(final.draft.private_key_file).toEqual({ action: "keep" });
    });

    it("distinguishes explicit optional-group removal from omitted sections", () => {
      const editor = beginConfigurationEdit(state);
      editor.draft.local_acquisition = null;
      expect(dirtySections(editor)).toEqual(["music_acquisition"]);
      expect(sectionUpdate(editor.draft, "music_acquisition")).toEqual({
        section: "music_acquisition",
        local_acquisition: null,
      });
      expect(sectionUpdate(editor.draft, "tls")).not.toHaveProperty("local_acquisition");
    });

    it("keeps hidden unrelated controls disabled during a section save", () => {
      const markup = renderToStaticMarkup(
        <BackendConfigurationForm
          editor={beginConfigurationEdit(state)}
          section="tls"
          disabled={false}
          busy={false}
          onChange={vi.fn()}
          onValidate={vi.fn()}
          onSave={vi.fn()}
          onDiscard={vi.fn()}
        />,
      );
      expect(markup).toMatch(/<fieldset disabled="" hidden="">/);
      expect(markup).toContain("Save changes");
      expect(markup).not.toContain("Validate section");
    });

    it("discards only the selected section and preserves other drafts", () => {
      const editor = beginConfigurationEdit(state);
      editor.draft.origin = "https://localhost:9443";
      editor.draft.private_key_file = { action: "replace", value: "retained-reference" };
      const next = discardConfigurationSection(editor, "general");
      expect(next.draft.origin).toBe(state.saved.origin);
      expect(next.expectedRevision).toBe(editor.expectedRevision);
      expect(dirtySections(next)).toEqual(["tls"]);
      expect(next.draft.private_key_file).toEqual(editor.draft.private_key_file);
      expect(next.baseline).toBe(editor.baseline);
    });
  });

  describe("reload confirmation", () => {
    it("waits for the exact active revision instead of treating acceptance as success", async () => {
      const next = { ...state, active_revision: "b".repeat(64) };
      const load = vi.fn().mockResolvedValueOnce(state).mockResolvedValueOnce(next);
      expect(await confirmConfigurationReload(next.active_revision, load, async () => {})).toBe(
        next,
      );
      expect(load).toHaveBeenCalledTimes(2);
    });

    it("reports activation failure and login requirements without retries", async () => {
      await expect(
        confirmConfigurationReload(
          "b",
          async () => ({
            ...state,
            reload: { status: "failed", revision: "b", message: "Replacement listener failed." },
          }),
          async () => {},
        ),
      ).rejects.toThrow("Replacement listener failed");
      const load = vi.fn().mockRejectedValue(new ApiError(401, "Sign in again."));
      await expect(confirmConfigurationReload("b", load, async () => {})).rejects.toThrow(
        "Sign in again",
      );
      expect(load).toHaveBeenCalledTimes(1);
    });

    it("bounds failed reads and never issues an activation command", async () => {
      const load = vi.fn().mockRejectedValue(new Error("Connection lost."));
      await expect(confirmConfigurationReload("b", load, async () => {})).rejects.toThrow(
        "The reload command was not retried.",
      );
      expect(load).toHaveBeenCalledTimes(10);
    });
  });
});
