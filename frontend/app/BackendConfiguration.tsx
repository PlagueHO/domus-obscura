import { useEffect, useState } from "react";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { BackendConfigurationResult } from "../src/generated/BackendConfigurationResult";
import { ApiError, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Badge, Button, Card } from "./components/ui";
import { BackendConfigurationForm } from "./BackendConfigurationForm";
import {
  beginConfigurationEdit,
  changeConfigurationDraft,
  configurationErrors,
  type ConfigurationEditor,
} from "./backend-configuration-state";

const configured = (value: boolean) => (value ? "Configured · value hidden" : "Not configured");

export function BackendConfiguration() {
  const resource = useResource(
    (signal) => request<BackendConfigurationState>("/api/system/configuration", { signal }),
    "backend-configuration",
  );
  const [selected, setSelected] = useState<"active" | "saved">("active");
  const [editor, setEditor] = useState<ConfigurationEditor>();
  const [confirmed, setConfirmed] = useState<BackendConfigurationState>();
  const [notice, setNotice] = useState("");
  const command = useCommand();
  useEffect(() => setConfirmed(undefined), [resource.data]);
  const state = confirmed ?? resource.data;
  const configuration = state?.[selected];
  const fresh = !resource.error && !resource.loading;
  const readOnly =
    !state || state.active.system_settings_read_only || state.saved.system_settings_read_only;
  async function submit(action: "validate" | "save") {
    if (!editor || editor.conflict || !fresh || readOnly) return;
    await command.run(async () => {
      setNotice("");
      try {
        const result = await request<BackendConfigurationResult>(
          `/api/system/configuration/${action}`,
          { body: { expected_revision: editor.expectedRevision, draft: editor.draft } },
        );
        if (result.errors.length) {
          setEditor(configurationErrors(editor, result.errors));
        } else if (action === "validate") {
          setEditor(configurationErrors(editor, []));
        } else if (result.state) {
          setConfirmed(result.state);
          setEditor(undefined);
          setNotice(
            "Backend configuration saved and passively validated. It is not activated. Restart the API to apply it.",
          );
          resource.refresh();
        } else {
          throw new Error(
            "The save response has no saved state. Reload and review before retrying.",
          );
        }
      } catch (error) {
        if (action === "save" || (error instanceof ApiError && error.status === 409)) {
          setEditor({ ...editor, validated: false, conflict: true });
        }
        throw error;
      }
    });
  }

  return (
    <Card title="Backend configuration">
      <p>
        This view covers the server JSON settings. Secret-file paths and Entra identifiers stay
        hidden. Backend JSON changes require an API restart. Saving does not activate them.
      </p>
      <Alert error={resource.error} />
      <Alert error={command.error} />
      {notice && <p role="status">{notice}</p>}
      <Button
        variant="secondary"
        disabled={resource.loading || command.busy}
        onClick={resource.refresh}
      >
        {resource.loading ? "Loading backend configuration…" : "Reload backend configuration"}
      </Button>
      {state && (
        <p>
          {readOnly
            ? "Backend configuration is read-only. The operator must enable editing outside the UI."
            : "The configured administrator can edit backend configuration. System settings access cannot be changed here."}
        </p>
      )}
      {!editor && !readOnly && (
        <Button
          disabled={!fresh || command.busy}
          onClick={() => state && setEditor(beginConfigurationEdit(state))}
        >
          Edit backend configuration
        </Button>
      )}
      {editor && (
        <BackendConfigurationForm
          editor={editor}
          disabled={command.busy || !fresh || readOnly}
          busy={command.busy}
          onChange={(draft) => setEditor(changeConfigurationDraft(editor, draft))}
          onValidate={() => void submit("validate")}
          onSave={() => void submit("save")}
          onDiscard={() => setEditor(undefined)}
        />
      )}
      {Boolean(resource.error) && state && (
        <p role="status">Last loaded configuration. The current saved file could not be checked.</p>
      )}
      {resource.loading && state && (
        <p role="status">
          Checking the saved file. Values from the last successful read remain below.
        </p>
      )}
      {state && configuration && (
        <>
          {!resource.error && !resource.loading && (
            <p role="status">
              {state.restart_required
                ? "Saved JSON differs from the active configuration. It is not active. A restart and full startup validation are required."
                : "Saved JSON matches the active configuration."}
            </p>
          )}
          <label>
            Configuration version
            <select
              value={selected}
              onChange={(event) => setSelected(event.target.value === "saved" ? "saved" : "active")}
            >
              <option value="active">Active in this process</option>
              <option value="saved">Saved JSON · not activated by this view</option>
            </select>
          </label>
          <p>
            <Badge>{selected === "active" ? "Active configuration" : "Saved configuration"}</Badge>
            {selected === "saved" && state.saved_requires_validation && (
              <> Saved values have not passed startup validation.</>
            )}
            {selected === "saved" && !state.saved_requires_validation && state.restart_required && (
              <>
                {" "}
                Saved values passed passive validation. They are not active. Startup checks remain
                required.
              </>
            )}
          </p>
          <dl className="properties">
            <dt>Environment</dt>
            <dd>{configuration.environment}</dd>
            <dt>Listen address</dt>
            <dd>{configuration.listen}</dd>
            <dt>Browser origin</dt>
            <dd>{configuration.origin}</dd>
            <dt>Local database</dt>
            <dd>{configuration.database}</dd>
            <dt>Browser assets</dt>
            <dd>{configuration.browser_assets ?? "Not configured"}</dd>
            <dt>System settings access</dt>
            <dd>
              {configuration.system_settings_read_only
                ? "View only"
                : "Library policy can be edited"}
            </dd>
            <dt>Discovery providers</dt>
            <dd>{configuration.services.discovery.join(", ") || "Disabled"}</dd>
            <dt>Metadata providers</dt>
            <dd>{configuration.services.metadata.join(", ") || "Disabled"}</dd>
            <dt>Artwork providers</dt>
            <dd>{configuration.services.artwork.join(", ") || "Disabled"}</dd>
            <dt>Acquisition providers</dt>
            <dd>{configuration.services.acquisition.join(", ") || "Disabled"}</dd>
            <dt>Entra tenant</dt>
            <dd>{configured(configuration.tenant_id_configured)}</dd>
            <dt>Authorized Entra object</dt>
            <dd>{configured(configuration.object_id_configured)}</dd>
            <dt>Entra client</dt>
            <dd>{configured(configuration.client_id_configured)}</dd>
            <dt>Entra secret file</dt>
            <dd>{configured(configuration.client_secret_file_configured)}</dd>
            <dt>TLS certificate file</dt>
            <dd>{configuration.certificate_file}</dd>
            <dt>TLS private-key file</dt>
            <dd>{configured(configuration.private_key_file_configured)}</dd>
            <dt>Provider cache</dt>
            <dd>{configuration.cache ? "Shared private file cache" : "Legacy SQLite cache"}</dd>
            {configuration.cache && (
              <>
                <dt>Cache root</dt>
                <dd>{configuration.cache.root}</dd>
                <dt>Maximum cache item</dt>
                <dd>{configuration.cache.item_bytes.toLocaleString()} bytes</dd>
                <dt>Maximum cache storage</dt>
                <dd>{configuration.cache.total_bytes.toLocaleString()} bytes</dd>
                <dt>Maximum cache entries</dt>
                <dd>{configuration.cache.entries.toLocaleString()}</dd>
              </>
            )}
            <dt>Development acquisition</dt>
            <dd>
              {configuration.local_acquisition ? "Configured · development only" : "Disabled"}
            </dd>
            {configuration.local_acquisition && (
              <>
                <dt>Private staging root</dt>
                <dd>{configuration.local_acquisition.staging_root}</dd>
                <dt>Soulseek credential file</dt>
                <dd>{configured(configuration.local_acquisition.credentials_file_configured)}</dd>
                <dt>Soulseek server</dt>
                <dd>{configuration.local_acquisition.server}</dd>
                <dt>Plaintext login acknowledgement</dt>
                <dd>
                  {configuration.local_acquisition.acknowledge_plaintext_login
                    ? "Acknowledged"
                    : "Not acknowledged"}
                </dd>
              </>
            )}
          </dl>
          <details>
            <summary>Configuration revisions</summary>
            <dl className="properties">
              <dt>Active revision</dt>
              <dd>
                <code>{state.active_revision}</code>
              </dd>
              <dt>Saved revision</dt>
              <dd>
                <code>{state.saved_revision}</code>
              </dd>
            </dl>
          </details>
        </>
      )}
    </Card>
  );
}
