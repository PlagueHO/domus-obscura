import { useEffect, useRef, type ReactNode } from "react";
import type { BackendConfigurationDraft } from "../src/generated/BackendConfigurationDraft";
import type { ConfigurationReplacement } from "../src/generated/ConfigurationReplacement";
import type { ConfigurationEditor } from "./backend-configuration-state";
import { Button, Check } from "./components/ui";

type Props = {
  editor: ConfigurationEditor;
  disabled: boolean;
  busy: boolean;
  onChange: (draft: BackendConfigurationDraft) => void;
  onValidate: () => void;
  onSave: () => void;
  onDiscard: () => void;
};

export function BackendConfigurationForm({
  editor,
  disabled,
  busy,
  onChange,
  onValidate,
  onSave,
  onDiscard,
}: Props) {
  const errorSummary = useRef<HTMLDivElement>(null);
  const conflictSummary = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if (editor.errors.length) errorSummary.current?.focus();
    else if (editor.conflict) conflictSummary.current?.focus();
  }, [editor.errors, editor.conflict]);
  const draft = editor.draft;
  const update = <K extends keyof BackendConfigurationDraft>(
    field: K,
    value: BackendConfigurationDraft[K],
  ) => onChange({ ...draft, [field]: value });
  const field = (name: string, label: string, help: string, input: ReactNode) => {
    const errors = editor.errors.filter((error) => error.field === name);
    return (
      <label key={name}>
        {label}
        {input}
        <small>{help}</small>
        {errors.map((error) => (
          <span className="configuration-field-error" key={error.message}>
            {error.message}
          </span>
        ))}
      </label>
    );
  };
  const text = (
    name: string,
    label: string,
    value: string,
    change: (value: string) => void,
    help: string,
  ) =>
    field(
      name,
      label,
      help,
      <input
        name={name}
        value={value}
        onChange={(event) => change(event.target.value)}
        aria-invalid={editor.errors.some((error) => error.field === name)}
        required={name !== "browser_assets"}
        spellCheck={false}
      />,
    );
  const hidden = (
    name: string,
    label: string,
    value: ConfigurationReplacement,
    change: (value: ConfigurationReplacement) => void,
    clear = true,
  ) => (
    <div className="configuration-secret" key={name}>
      {field(
        name,
        label,
        "The saved value stays hidden. A blank replacement keeps the saved value.",
        <select
          name={`${name}.action`}
          value={value.action}
          onChange={(event) =>
            change(
              event.target.value === "replace"
                ? { action: "replace", value: "" }
                : event.target.value === "clear"
                  ? { action: "clear" }
                  : { action: "keep" },
            )
          }
        >
          <option value="keep">Keep saved value</option>
          <option value="replace">Replace value</option>
          {clear && <option value="clear">Remove saved value</option>}
        </select>,
      )}
      {value.action === "replace" && (
        <label>
          New {label.toLowerCase()}
          <input
            name={name}
            type="password"
            autoComplete="new-password"
            spellCheck={false}
            value={value.value}
            onChange={(event) => change({ action: "replace", value: event.target.value })}
          />
        </label>
      )}
    </div>
  );

  return (
    <form
      aria-label="Edit backend configuration"
      onSubmit={(event) => {
        event.preventDefault();
        onSave();
      }}
    >
      <p>
        All changes require an API restart. Saving does not activate settings or start work. System
        settings access stays operator-controlled.
      </p>
      {editor.errors.length > 0 && (
        <div role="alert" className="alert" tabIndex={-1} ref={errorSummary}>
          <strong>Configuration was not saved.</strong>
          <ul>
            {editor.errors.map((error) => (
              <li key={`${error.field}:${error.message}`}>
                {error.field}: {error.message}
              </li>
            ))}
          </ul>
        </div>
      )}
      {editor.conflict && (
        <p role="alert" tabIndex={-1} ref={conflictSummary}>
          The saved file changed or the save result is uncertain. Your draft is retained. Reload and
          review the saved values, then discard this draft before starting a new edit. This form
          will not retry automatically.
        </p>
      )}
      <fieldset disabled={disabled}>
        <legend>Host · restart required</legend>
        {field(
          "environment",
          "Environment",
          "Production requires Entra. Development permits only direct loopback HTTPS.",
          <select
            value={draft.environment}
            onChange={(event) => update("environment", event.target.value)}
          >
            <option value="production">Production</option>
            <option value="development">Development</option>
          </select>,
        )}
        {text(
          "database",
          "Local database",
          draft.database,
          (value) => update("database", value),
          "Absolute path to an existing local database. Migrate it separately.",
        )}
        {text(
          "listen",
          "Listen address",
          draft.listen,
          (value) => update("listen", value),
          "Loopback IP address and nonzero port. Public operation is not enabled.",
        )}
        {text(
          "origin",
          "Browser origin",
          draft.origin,
          (value) => update("origin", value),
          "HTTPS origin with the listener port. No path, query, or credentials.",
        )}
        {text(
          "browser_assets",
          "Browser assets",
          draft.browser_assets ?? "",
          (value) => update("browser_assets", value || null),
          "Optional absolute path to existing built browser assets. Leave blank to disable.",
        )}
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Providers · restart required</legend>
        <p>
          Enable installed providers separately for each service. No provider requests run here.
        </p>
        {(
          [
            ["discovery", "Discovery · MusicBrainz", "musicbrainz"],
            ["metadata", "Metadata · MusicBrainz", "musicbrainz"],
            ["artwork", "Artwork · Cover Art Archive", "cover-art-archive"],
            ["acquisition", "Acquisition · Soulseek", "soulseek"],
          ] as const
        ).map(([role, label, provider]) => (
          <Check
            key={role}
            checked={draft.services[role].includes(provider)}
            onChange={(enabled) =>
              update("services", { ...draft.services, [role]: enabled ? [provider] : [] })
            }
          >
            {label}
          </Check>
        ))}
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Provider cache · restart required</legend>
        <Check
          checked={draft.cache !== null}
          onChange={(enabled) =>
            update(
              "cache",
              enabled
                ? { root: "", item_bytes: 9437184, total_bytes: 134217728, entries: 64 }
                : null,
            )
          }
        >
          Use the shared private file cache
        </Check>
        <p>Disabling this setting selects legacy SQLite storage. It does not delete cache files.</p>
        {draft.cache && (
          <>
            {text(
              "cache.root",
              "Cache root",
              draft.cache.root,
              (root) => draft.cache && update("cache", { ...draft.cache, root }),
              "Existing private local root. Set up its native binding separately.",
            )}
            {(
              [
                ["item_bytes", "Maximum cache item (bytes)", 1, 33554432],
                ["total_bytes", "Maximum cache storage (bytes)", 1, 1073741824],
                ["entries", "Maximum cache entries", 1, 4096],
              ] as const
            ).map(([key, label, min, max]) =>
              field(
                `cache.${key}`,
                label,
                key === "total_bytes"
                  ? "Reserve at least two items, each with a 96-byte header."
                  : `Use a whole number from ${min} to ${max}.`,
                <input
                  key={key}
                  type="number"
                  required
                  min={min}
                  max={max}
                  step={1}
                  value={draft.cache?.[key] ?? ""}
                  onChange={(event) =>
                    draft.cache &&
                    update("cache", { ...draft.cache, [key]: Number(event.target.value) })
                  }
                />,
              ),
            )}
          </>
        )}
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Entra identity · restart required</legend>
        <p>
          Production authorizes one tenant/object pair. Changing environment can require explicit
          removal or replacement of these fields.
        </p>
        {hidden("tenant_id", "Entra tenant", draft.tenant_id, (value) =>
          update("tenant_id", value),
        )}
        {hidden("object_id", "Authorized Entra object", draft.object_id, (value) =>
          update("object_id", value),
        )}
        {hidden("client_id", "Entra client", draft.client_id, (value) =>
          update("client_id", value),
        )}
        {hidden("client_secret_file", "Entra secret file", draft.client_secret_file, (value) =>
          update("client_secret_file", value),
        )}
        <small>
          File references only. This form does not accept passwords, tokens, or private-key
          contents.
        </small>
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>TLS · restart required</legend>
        {text(
          "certificate_file",
          "TLS certificate file",
          draft.certificate_file,
          (value) => update("certificate_file", value),
          "Absolute path to an existing PEM certificate. Validation checks the key pair, chain, hostname, and validity.",
        )}
        {hidden(
          "private_key_file",
          "TLS private-key file",
          draft.private_key_file,
          (value) => update("private_key_file", value),
          false,
        )}
      </fieldset>
      <fieldset disabled={disabled}>
        <legend>Development acquisition · restart required</legend>
        <Check
          checked={draft.local_acquisition !== null}
          onChange={(enabled) =>
            update(
              "local_acquisition",
              enabled
                ? {
                    staging_root: "",
                    credentials_file: { action: "keep" },
                    server: "",
                    acknowledge_plaintext_login: false,
                  }
                : null,
            )
          }
        >
          Configure development acquisition
        </Check>
        <p>Disabling this group removes its saved references. It does not delete staging files.</p>
        {draft.local_acquisition && (
          <>
            {text(
              "local_acquisition.staging_root",
              "Private staging root",
              draft.local_acquisition.staging_root,
              (staging_root) =>
                draft.local_acquisition &&
                update("local_acquisition", { ...draft.local_acquisition, staging_root }),
              "Existing private local staging root with a valid native binding.",
            )}
            {hidden(
              "local_acquisition.credentials_file",
              "Soulseek credential file",
              draft.local_acquisition.credentials_file,
              (credentials_file) =>
                draft.local_acquisition &&
                update("local_acquisition", { ...draft.local_acquisition, credentials_file }),
              false,
            )}
            {text(
              "local_acquisition.server",
              "Soulseek server",
              draft.local_acquisition.server,
              (server) =>
                draft.local_acquisition &&
                update("local_acquisition", { ...draft.local_acquisition, server }),
              "Public IPv4 address and nonzero port. Validation does not connect.",
            )}
            <Check
              checked={draft.local_acquisition.acknowledge_plaintext_login}
              onChange={(acknowledge_plaintext_login) =>
                draft.local_acquisition &&
                update("local_acquisition", {
                  ...draft.local_acquisition,
                  acknowledge_plaintext_login,
                })
              }
            >
              I acknowledge that Soulseek login uses plaintext.
            </Check>
          </>
        )}
      </fieldset>
      <p role="status">
        {editor.validated
          ? "This draft passed passive validation. It is not saved or active. Startup will check it again."
          : "This draft has not passed passive validation."}
      </p>
      <div className="actions">
        <Button variant="secondary" disabled={disabled || editor.conflict} onClick={onValidate}>
          Validate backend configuration
        </Button>
        <Button type="submit" disabled={disabled || editor.conflict}>
          Save backend configuration
        </Button>
        <Button variant="secondary" disabled={busy} onClick={onDiscard}>
          Discard backend draft
        </Button>
      </div>
    </form>
  );
}
