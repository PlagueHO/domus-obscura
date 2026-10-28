import { useEffect, useRef, type ReactNode } from "react";
import type { BackendConfigurationDraft } from "../src/generated/BackendConfigurationDraft";
import type { ConfigurationReplacement } from "../src/generated/ConfigurationReplacement";
import type { ConfigurationSection } from "../src/generated/ConfigurationSection";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { BackendSharingDraft } from "../src/generated/BackendSharingDraft";
import type { ShareLimits } from "../src/generated/ShareLimits";
import type { ConfigurationEditor } from "./backend-configuration-state";
import { newSharingDraft } from "./backend-configuration-state";
import { SharingFolders } from "./SharingFolders";
import { Button, Check } from "./components/ui";

type Props = {
  editor: ConfigurationEditor;
  disabled: boolean;
  busy: boolean;
  onChange: (draft: BackendConfigurationDraft) => void;
  onValidate: () => void;
  onSave: () => void;
  onDiscard: () => void;
  section?: ConfigurationSection;
  onSaveAll?: (() => void) | undefined;
  changed?: boolean;
  saved?: BackendConfigurationState["saved"];
};

export function BackendConfigurationForm({
  editor,
  disabled,
  busy,
  onChange,
  onValidate,
  onSave,
  onDiscard,
  section,
  onSaveAll,
  changed = true,
  saved,
}: Props) {
  const errorSummary = useRef<HTMLDivElement>(null);
  const conflictSummary = useRef<HTMLParagraphElement>(null);
  useEffect(() => {
    if (editor.errors.length) errorSummary.current?.focus();
    else if (editor.conflict) conflictSummary.current?.focus();
  }, [editor.errors, editor.conflict]);
  const draft = editor.draft;
  const savedSecrets: Record<string, boolean | undefined> = {
    tenant_id: saved?.tenant_id_configured,
    object_id: saved?.object_id_configured,
    client_id: saved?.client_id_configured,
    client_secret_file: saved?.client_secret_file_configured,
    private_key_file: saved?.private_key_file_configured,
    "local_acquisition.credentials_file": saved?.local_acquisition?.credentials_file_configured,
    "production_worker.acquisition.credentials_file":
      saved?.production_worker?.acquisition?.credentials_file_configured,
    "production_worker.sharing.account.credentials_file":
      saved?.production_worker?.sharing?.account?.credentials_file_configured,
  };
  const visible = (value: ConfigurationSection) => section === undefined || section === value;
  const update = <K extends keyof BackendConfigurationDraft>(
    field: K,
    value: BackendConfigurationDraft[K],
  ) => onChange({ ...draft, [field]: value });
  const sharing = draft.production_worker?.sharing;
  const updateSharing = (value: BackendSharingDraft | null) =>
    update(
      "production_worker",
      draft.production_worker
        ? { ...draft.production_worker, sharing: value }
        : value
          ? { queue_capacity: 64, acquisition: null, sharing: value }
          : null,
    );
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
        savedSecrets[name] === false
          ? "Not configured. Choose Replace value to add it."
          : savedSecrets[name]
            ? "Configured. The saved value stays hidden."
            : "The saved value stays hidden.",
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
          <small>Leave blank to keep the saved value.</small>
        </label>
      )}
    </div>
  );

  return (
    <form
      className={section ? "configuration-form" : undefined}
      aria-label="Edit backend configuration"
      onSubmit={(event) => {
        event.preventDefault();
        onSave();
      }}
    >
      {!section && <p>Saving does not activate settings. Reload the API to apply changes.</p>}
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
          The saved settings changed or the save could not be confirmed. Your draft is kept. Refresh
          and review the saved values, then discard all drafts to start again. Nothing was retried.
        </p>
      )}
      <fieldset disabled={disabled || !visible("general")} hidden={!visible("general")}>
        <legend>General</legend>
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
      <fieldset
        disabled={disabled || !visible("music_providers")}
        hidden={!visible("music_providers")}
      >
        <legend>Providers</legend>
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
      <fieldset disabled={disabled || !visible("storage")} hidden={!visible("storage")}>
        <legend>Storage/cache</legend>
        {text(
          "database",
          "Local database",
          draft.database,
          (value) => update("database", value),
          "Absolute path to an existing local database. Migrate it separately.",
        )}
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
      <fieldset disabled={disabled || !visible("identity")} hidden={!visible("identity")}>
        <legend>Entra identity</legend>
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
      <fieldset disabled={disabled || !visible("tls")} hidden={!visible("tls")}>
        <legend>TLS</legend>
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
      <fieldset
        disabled={disabled || !visible("music_acquisition")}
        hidden={!visible("music_acquisition")}
      >
        <legend>Development acquisition</legend>
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
      <fieldset
        disabled={disabled || !visible("music_acquisition")}
        hidden={!visible("music_acquisition")}
      >
        <legend>Production worker</legend>
        <Check
          checked={draft.production_worker !== null}
          onChange={(enabled) =>
            update(
              "production_worker",
              enabled
                ? {
                    queue_capacity: 64,
                    acquisition: null,
                    sharing: draft.production_worker?.sharing ?? null,
                  }
                : draft.production_worker?.sharing
                  ? { ...draft.production_worker, acquisition: null }
                  : null,
            )
          }
        >
          Configure the separate production worker
        </Check>
        <p>
          Saving does not restart the worker. Restart it separately at a safe boundary. Do not
          enable development acquisition at the same time.
        </p>
        {draft.production_worker && (
          <>
            {field(
              "production_worker.queue_capacity",
              "Queue capacity",
              "Allow 1 through 1024 unfinished requests. Waiting requests count. A lower limit never deletes saved requests.",
              <input
                type="number"
                min={1}
                max={1024}
                step={1}
                required
                value={draft.production_worker.queue_capacity}
                onChange={(event) =>
                  draft.production_worker &&
                  update("production_worker", {
                    ...draft.production_worker,
                    queue_capacity: Number(event.target.value),
                  })
                }
              />,
            )}
            <Check
              checked={draft.production_worker.acquisition !== null}
              onChange={(enabled) =>
                draft.production_worker &&
                update("production_worker", {
                  ...draft.production_worker,
                  acquisition: enabled
                    ? {
                        staging_root: "",
                        credentials_file: { action: "keep" },
                        server: "",
                        acknowledge_plaintext_login: false,
                      }
                    : null,
                })
              }
            >
              Configure approved acquisition and publication
            </Check>
            <p>
              Without acquisition settings, the worker can run read-only index scans and configured
              sharing. It does not need staging for sharing.
            </p>
            {draft.production_worker.acquisition && (
              <>
                {text(
                  "production_worker.acquisition.staging_root",
                  "Worker staging root",
                  draft.production_worker.acquisition.staging_root,
                  (staging_root) =>
                    draft.production_worker?.acquisition &&
                    update("production_worker", {
                      ...draft.production_worker,
                      acquisition: { ...draft.production_worker.acquisition, staging_root },
                    }),
                  "Existing private staging root with its native binding.",
                )}
                {hidden(
                  "production_worker.acquisition.credentials_file",
                  "Worker Soulseek credential file",
                  draft.production_worker.acquisition.credentials_file,
                  (credentials_file) =>
                    draft.production_worker?.acquisition &&
                    update("production_worker", {
                      ...draft.production_worker,
                      acquisition: { ...draft.production_worker.acquisition, credentials_file },
                    }),
                  false,
                )}
                {text(
                  "production_worker.acquisition.server",
                  "Worker Soulseek server",
                  draft.production_worker.acquisition.server,
                  (server) =>
                    draft.production_worker?.acquisition &&
                    update("production_worker", {
                      ...draft.production_worker,
                      acquisition: { ...draft.production_worker.acquisition, server },
                    }),
                  "Public IPv4 address and port. Validation does not connect.",
                )}
                <Check
                  checked={draft.production_worker.acquisition.acknowledge_plaintext_login}
                  onChange={(acknowledge_plaintext_login) =>
                    draft.production_worker?.acquisition &&
                    update("production_worker", {
                      ...draft.production_worker,
                      acquisition: {
                        ...draft.production_worker.acquisition,
                        acknowledge_plaintext_login,
                      },
                    })
                  }
                >
                  I acknowledge that the worker Soulseek login uses plaintext.
                </Check>
              </>
            )}
          </>
        )}
      </fieldset>
      <fieldset disabled={disabled || !visible("music_sharing")} hidden={!visible("music_sharing")}>
        <legend>Soulseek sharing</legend>
        <p>
          Sharing is off by default. Saving or reloading the API does not start sharing. Restart the
          separate worker at a safe boundary to apply saved changes. Only MP3, FLAC and JPEG files
          can be shared.
        </p>
        {field(
          "production_worker.sharing.policy.access",
          "Who can access shared music",
          "The same access policy controls browsing, search, queues and uploads.",
          <select
            name="production_worker.sharing.policy.access"
            value={sharing?.policy.access ?? "none"}
            onChange={(event) => {
              const access = event.target.value as "none" | "all" | "allowlist";
              const value = sharing ?? newSharingDraft();
              updateSharing({ ...value, policy: { ...value.policy, access } });
            }}
          >
            <option value="none">None — sharing off</option>
            <option value="all">All Soulseek users</option>
            <option value="allowlist">Specific allowlist</option>
          </select>,
        )}
        {!sharing && (
          <Button
            variant="secondary"
            disabled={disabled}
            onClick={() => updateSharing(newSharingDraft())}
          >
            Set up sharing while disabled
          </Button>
        )}
        {sharing && (
          <>
            {sharing.policy.access === "allowlist" &&
              field(
                "production_worker.sharing.policy.allowlist",
                "Allowed Soulseek usernames",
                "One exact, case-sensitive username per line. Maximum 256 names; 1 through 30 printable ASCII characters per name.",
                <textarea
                  name="production_worker.sharing.policy.allowlist"
                  value={sharing.policy.allowlist.join("\n")}
                  rows={5}
                  spellCheck={false}
                  onChange={(event) =>
                    updateSharing({
                      ...sharing,
                      policy: { ...sharing.policy, allowlist: event.target.value.split("\n") },
                    })
                  }
                />,
              )}
            {visible("music_sharing") && (
              <SharingFolders
                roots={sharing.policy.roots}
                disabled={disabled}
                onChange={(roots) =>
                  updateSharing({ ...sharing, policy: { ...sharing.policy, roots } })
                }
              />
            )}
            <Check
              checked={sharing.account !== null}
              onChange={(enabled) =>
                updateSharing({
                  ...sharing,
                  account: enabled
                    ? {
                        credentials_file: { action: "keep" },
                        server: "",
                        acknowledge_plaintext_login: false,
                      }
                    : null,
                })
              }
            >
              Configure an explicit sharing account
            </Check>
            <p>
              Otherwise use the production acquisition account. Sharing and acquisition use one
              session. Explicit settings must match acquisition settings when both exist. A
              sharing-only worker does not need a staging root.
            </p>
            {sharing.account && (
              <>
                {hidden(
                  "production_worker.sharing.account.credentials_file",
                  "Sharing Soulseek credential file",
                  sharing.account.credentials_file,
                  (credentials_file) =>
                    sharing.account &&
                    updateSharing({
                      ...sharing,
                      account: { ...sharing.account, credentials_file },
                    }),
                  false,
                )}
                {text(
                  "production_worker.sharing.account.server",
                  "Sharing Soulseek server",
                  sharing.account.server,
                  (server) =>
                    sharing.account &&
                    updateSharing({
                      ...sharing,
                      account: { ...sharing.account, server },
                    }),
                  "Public IPv4 address and nonzero port. Validation does not connect.",
                )}
                <Check
                  checked={sharing.account.acknowledge_plaintext_login}
                  onChange={(acknowledge_plaintext_login) =>
                    sharing.account &&
                    updateSharing({
                      ...sharing,
                      account: { ...sharing.account, acknowledge_plaintext_login },
                    })
                  }
                >
                  I acknowledge that Soulseek login uses plaintext.
                </Check>
              </>
            )}
            <Check
              checked={sharing.listen !== null}
              onChange={(enabled) => updateSharing({ ...sharing, listen: enabled ? "" : null })}
            >
              Configure an incoming peer listener
            </Check>
            {sharing.listen !== null &&
              text(
                "production_worker.sharing.listen",
                "Sharing peer listen address",
                sharing.listen,
                (listen) => updateSharing({ ...sharing, listen }),
                "Explicit IPv4 address and nonzero port. No automatic port mapping or distributed-child acceptance. Validation does not open a listener.",
              )}
            <details className="configuration-details configuration-sharing-limits">
              <summary>Sharing resource limits</summary>
              <p>
                All limits are finite. Sizes use bytes; timeouts use seconds. Related limits must be
                consistent.
              </p>
              <div className="configuration-sharing-limit-fields">
                {sharingLimitFields.map(([name, label, min, max, unit]) =>
                  field(
                    `production_worker.sharing.policy.limits.${name}`,
                    label,
                    `${min.toLocaleString("en")} through ${max.toLocaleString("en")} ${unit}.${name === "catalog_bytes" ? " Estimated entry data only. This excludes database pages, indexes, WAL and unrelated data; it is not a disk quota." : ""}`,
                    <input
                      type="number"
                      name={`production_worker.sharing.policy.limits.${name}`}
                      min={min}
                      max={max}
                      step={1}
                      required
                      value={sharing.policy.limits[name]}
                      onChange={(event) =>
                        updateSharing({
                          ...sharing,
                          policy: {
                            ...sharing.policy,
                            limits: {
                              ...sharing.policy.limits,
                              [name]: Number(event.target.value),
                            },
                          },
                        })
                      }
                    />,
                  ),
                )}
              </div>
            </details>
          </>
        )}
      </fieldset>
      {editor.validated && (
        <p role="status">
          Changes checked. Save them when you are ready. Reload performs full startup validation.
        </p>
      )}
      <div className="actions configuration-form-actions">
        {!section && (
          <Button variant="secondary" disabled={disabled || editor.conflict} onClick={onValidate}>
            Validate backend configuration
          </Button>
        )}
        <Button type="submit" disabled={disabled || editor.conflict || !changed}>
          {section ? "Save changes" : "Save backend configuration"}
        </Button>
        {onSaveAll && (
          <Button variant="secondary" disabled={disabled || editor.conflict} onClick={onSaveAll}>
            Save all changed sections
          </Button>
        )}
        {(!section || changed || editor.conflict) && (
          <Button variant="ghost" disabled={busy} onClick={onDiscard}>
            {section
              ? editor.conflict
                ? "Discard all drafts"
                : "Discard changes"
              : "Discard backend draft"}
          </Button>
        )}
      </div>
    </form>
  );
}

const sharingLimitFields: [keyof ShareLimits, string, number, number, string][] = [
  ["upload_slots", "Concurrent uploads", 1, 8, "uploads"],
  ["queued_files", "Queued files", 1, 1024, "files"],
  ["queued_per_peer", "Queued files per peer", 1, 128, "files"],
  ["buffer_bytes", "Transfer buffer", 4096, 262144, "bytes"],
  ["roots", "Selected share roots", 1, 64, "roots"],
  ["catalog_files", "Complete catalog files", 1, 2000000, "files"],
  ["catalog_bytes", "Encoded catalog entry budget", 1024, 1073741824, "bytes"],
  ["expanded_bytes", "Expanded complete browse response", 1024, 536870912, "bytes"],
  ["compressed_bytes", "Compressed complete browse response", 1024, 134217728, "bytes"],
  ["spool_bytes", "Private browse spool storage", 1024, 1073741824, "bytes"],
  ["peer_connections", "Peer connections", 1, 24, "connections"],
  ["pending_handshakes", "Pending handshakes", 1, 8, "handshakes"],
  ["requests_per_minute", "Requests per minute", 1, 600, "requests per minute"],
  ["search_results", "Search results", 1, 1000, "results"],
  ["concurrent_browses", "Concurrent large browse responses", 1, 4, "responses"],
  ["idle_seconds", "Idle timeout", 1, 30, "seconds"],
  ["operation_seconds", "Operation timeout", 1, 3600, "seconds"],
];
