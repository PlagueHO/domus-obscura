import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { BackendCertificateStatus } from "../src/generated/BackendCertificateStatus";
import type { BackendConfigurationResult } from "../src/generated/BackendConfigurationResult";
import type { ConfigurationSection } from "../src/generated/ConfigurationSection";
import type { ConfigurationReloadState } from "../src/generated/ConfigurationReloadState";
import type { WorkerStatus } from "../src/generated/WorkerStatus";
import type { LocalStatus } from "../src/generated/LocalStatus";
import { ApiError, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button } from "./components/ui";
import { Icon } from "./components/Media";
import { BackendConfigurationForm } from "./BackendConfigurationForm";
import { ArtistPhotoBindings } from "./ArtistPhotoBindings";
import {
  acceptConfigurationSave,
  beginConfigurationEdit,
  changeConfigurationDraft,
  configurationErrors,
  configurationSections,
  dirtySections,
  discardConfigurationSection,
  sectionHasErrors,
  sectionUpdate,
  type ConfigurationEditor,
} from "./backend-configuration-state";

type Section = ConfigurationSection | "music_output" | "music_roots";
const configured = (value: boolean) => (value ? "Configured · value hidden" : "Not configured");

const RefreshContext = createContext<{ busy: boolean; refresh: () => void } | undefined>(undefined);

/** Settings card with the component-library header band and a shared refresh action. */
export function ConfigurationCard({ title, children }: { title: string; children: ReactNode }) {
  const refresh = useContext(RefreshContext);
  return (
    <section className="design-card configuration-section">
      <header className="design-card-header">
        <h2>{title}</h2>
        {refresh && (
          <Button
            variant="ghost"
            className="icon-button"
            aria-label="Refresh settings"
            title="Refresh settings"
            disabled={refresh.busy}
            onClick={refresh.refresh}
          >
            <Icon name="refresh" />
          </Button>
        )}
      </header>
      <div className="design-card-body">{children}</div>
    </section>
  );
}

/** A lost reload response must never cause a second activation command. */
export async function confirmConfigurationReload(
  revision: string,
  load: () => Promise<BackendConfigurationState>,
  wait: () => Promise<void>,
): Promise<BackendConfigurationState> {
  let failure = "The replacement host has not confirmed the saved revision.";
  for (let attempt = 0; attempt < 10; attempt++) {
    await wait();
    let state: BackendConfigurationState;
    try {
      state = await load();
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) throw error;
      failure = error instanceof Error ? error.message : "The host could not be reached.";
      continue;
    }
    if (state.active_revision === revision) return state;
    if (state.reload.status === "failed") throw new Error(state.reload.message);
  }
  throw new Error(
    `Reload could not be confirmed. ${failure} Select Refresh or sign in again. The reload command was not retried.`,
  );
}

export function BackendConfiguration({
  output,
  roots,
  otherDrafts = false,
  onRefreshRelated,
  relatedLoading = false,
}: {
  output?: ReactNode;
  roots?: ReactNode;
  otherDrafts?: boolean;
  onRefreshRelated?: () => void;
  relatedLoading?: boolean;
}) {
  const resource = useResource(
    (signal) => request<BackendConfigurationState>("/api/system/configuration", { signal }),
    "backend-configuration",
  );
  const [section, setSection] = useState<Section>(() => {
    const requested =
      typeof location === "undefined"
        ? null
        : new URLSearchParams(location.hash.split("?")[1]).get("section");
    return configurationSections.find(({ id }) => id === requested)?.id ?? "general";
  });
  const [draft, setEditor] = useState<ConfigurationEditor>();
  const [confirmed, setConfirmed] = useState<BackendConfigurationState>();
  const [notice, setNotice] = useState("");
  const [destination, setDestination] = useState("");
  const [uncertain, setUncertain] = useState(false);
  const abort = useRef<AbortController | undefined>(undefined);
  const command = useCommand();
  useEffect(() => {
    setConfirmed(undefined);
    setUncertain(false);
    setDestination("");
    setNotice("");
    setEditor((current) =>
      current && (dirtySections(current).length > 0 || current.conflict) ? current : undefined,
    );
  }, [resource.data]);
  useEffect(() => () => abort.current?.abort(), []);
  const state = confirmed ?? resource.data;
  const fresh = !resource.error && !resource.loading;
  const readOnly =
    !state || state.active.system_settings_read_only || state.saved.system_settings_read_only;
  const editor = draft ?? (state && !readOnly ? beginConfigurationEdit(state) : undefined);
  const changed = editor ? dirtySections(editor) : [];
  const currentSection = configurationSections.find(({ id }) => id === section);
  const backendSection = currentSection?.id;
  const reloading = state?.reload.status === "pending";

  async function submit(action: "validate" | "save", sections: ConfigurationSection[]) {
    if (
      !editor ||
      !sections.length ||
      editor.conflict ||
      !fresh ||
      readOnly ||
      uncertain ||
      reloading
    )
      return;
    await command.run(async () => {
      setNotice("");
      try {
        const result = await request<BackendConfigurationResult>(
          `/api/system/configuration/sections/${action}`,
          {
            body: {
              expected_revision: editor.expectedRevision,
              sections: sections.map((id) => sectionUpdate(editor.draft, id)),
            },
          },
        );
        if (result.errors.length) {
          setEditor(configurationErrors(editor, result.errors));
        } else if (action === "validate") {
          setEditor(configurationErrors(editor, []));
        } else if (result.state) {
          setConfirmed(result.state);
          setEditor(acceptConfigurationSave(editor, result.state, sections));
          setNotice("Changes saved.");
        } else {
          throw new Error(
            "The save response has no saved state. Refresh and review before retrying.",
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

  async function reload() {
    if (!state || changed.length || otherDrafts || !fresh || readOnly || reloading || uncertain)
      return;
    await command.run(async () => {
      setNotice("");
      const controller = new AbortController();
      abort.current = controller;
      setUncertain(true);
      const result = await request<ConfigurationReloadState>("/api/system/configuration/reload", {
        body: { expected_revision: state.saved_revision },
        signal: controller.signal,
      });
      if (result.status !== "pending")
        throw new Error("The host did not accept reload. Select Refresh before retrying.");
      setConfirmed({ ...state, reload: result });
      if (result.origin !== window.location.origin) {
        setDestination(result.origin);
        setNotice(
          "Open the saved address and check that the changes are active. You may need to sign in.",
        );
        return;
      }
      const active = await confirmConfigurationReload(
        result.revision,
        async () => {
          const observed = await request<BackendConfigurationState>("/api/system/configuration", {
            signal: AbortSignal.any([controller.signal, AbortSignal.timeout(3000)]),
          });
          setConfirmed(observed);
          return observed;
        },
        () =>
          new Promise<void>((resolve, reject) => {
            const timer = setTimeout(resolve, 1000);
            controller.signal.addEventListener(
              "abort",
              () => {
                clearTimeout(timer);
                reject(new Error("Reload confirmation stopped."));
              },
              { once: true },
            );
          }),
      ).catch((error: unknown) => {
        setNotice("");
        throw error;
      });
      setConfirmed(active);
      setUncertain(false);
      setNotice("The saved revision is active in the API. Separate workers are unchanged.");
      window.location.reload();
    });
  }

  const navigation = (id: Section, label: string) => (
    <Button
      key={id}
      variant="ghost"
      type="button"
      aria-current={section === id ? "page" : undefined}
      onClick={() => setSection(id)}
    >
      <span>{label}</span>
      {changed.some((value) => value === id) ? (
        <small>Unsaved</small>
      ) : (
        state?.changed_sections.some((value) => value === id) && <small>Reload needed</small>
      )}
      {editor &&
        configurationSections.some(
          ({ id: value }) => value === id && sectionHasErrors(editor, value),
        ) && <small>Error</small>}
    </Button>
  );

  return (
    <RefreshContext.Provider
      value={{
        busy: resource.loading || relatedLoading || command.busy,
        refresh: () => {
          resource.refresh();
          onRefreshRelated?.();
        },
      }}
    >
      <div className="configuration-messages">
        {notice && <p role="status">{notice}</p>}
        <Alert error={resource.error} />
        <Alert error={command.error} />
        {state?.reload.status === "failed" && !command.error && (
          <p role="alert">{state.reload.message}</p>
        )}
        {(state?.restart_required || reloading || uncertain) && (
          <div className="configuration-notice">
            <div role="status">
              <strong>
                {reloading
                  ? "Applying saved changes…"
                  : uncertain
                    ? "Reload needs confirmation"
                    : "Saved changes need a reload"}
              </strong>
              <p>
                {uncertain && !command.busy
                  ? "Refresh to check the server. The reload command was not retried."
                  : changed.length > 0 || otherDrafts
                    ? "Save or discard your unsaved changes before reloading."
                    : "Reload the API to apply saved settings. Separate workers stay unchanged."}
              </p>
            </div>
            <Button
              variant="secondary"
              disabled={
                !fresh ||
                readOnly ||
                command.busy ||
                reloading ||
                uncertain ||
                !state?.restart_required ||
                changed.length > 0 ||
                otherDrafts
              }
              onClick={() => void reload()}
            >
              Reload
            </Button>
          </div>
        )}
        {destination && (
          <p>
            <a href={destination} rel="noreferrer">
              Open saved browser address
            </a>
            . Expected revision: <code>{state?.saved_revision}</code>.
          </p>
        )}
        {state && readOnly && (
          <p role="status">
            These settings are read-only. The operator must enable editing outside the UI.
          </p>
        )}
        {Boolean(resource.error) && state && (
          <p role="status">
            The saved settings could not be checked. The last loaded values are shown.
          </p>
        )}
        {resource.loading && state && (
          <p role="status">Refreshing settings. The last loaded values are shown.</p>
        )}
      </div>
      <div className="configuration-layout">
        <label className="configuration-section-picker">
          Settings section
          <select
            value={section}
            onChange={(event) => {
              const value = event.target.value;
              const target = configurationSections.find(({ id }) => id === value);
              if (target) setSection(target.id);
              else if (value === "music_output" || value === "music_roots") setSection(value);
            }}
          >
            <optgroup label="Core application">
              {configurationSections
                .filter(({ group }) => group === "Core application")
                .map(({ id, title }) => (
                  <option key={id} value={id}>
                    {title}
                    {changed.includes(id) ? " (unsaved)" : ""}
                  </option>
                ))}
            </optgroup>
            <optgroup label="Media · Music">
              {configurationSections
                .filter(({ group }) => group === "Media · Music")
                .map(({ id, title }) => (
                  <option key={id} value={id}>
                    {title}
                    {changed.includes(id) ? " (unsaved)" : ""}
                  </option>
                ))}
              {output && <option value="music_output">Output policy</option>}
              {roots && <option value="music_roots">Library roots</option>}
            </optgroup>
          </select>
        </label>
        <nav className="configuration-navigation" aria-label="Configuration sections">
          <div className="configuration-group">
            <div className="configuration-group-label">
              <Icon name="settings" />
              Core application
            </div>
            {configurationSections
              .filter(({ group }) => group === "Core application")
              .map(({ id, title: label }) => navigation(id, label))}
          </div>
          <div className="configuration-group">
            <div className="configuration-group-label">
              <Icon name="music" />
              Media
            </div>
            <div className="configuration-subgroup">Music</div>
            {configurationSections
              .filter(({ group }) => group === "Media · Music")
              .map(({ id, title: label }) => navigation(id, label))}
            {output && navigation("music_output", "Output policy")}
            {roots && navigation("music_roots", "Library roots")}
          </div>
        </nav>
        <div className="configuration-content">
          <div hidden={!backendSection}>
            <ConfigurationCard title={currentSection?.title ?? "Settings"}>
              <p className="muted">{currentSection?.description}</p>
              {!state && !resource.error && <p role="status">Loading settings…</p>}
              {state &&
                fresh &&
                backendSection === "tls" &&
                state.active.environment !== "single_user" &&
                state.active.tls_mode === "azure_dns" && (
                  <ManagedCertificateStatus revision={state.active_revision} />
                )}
              {state && backendSection === "music_sharing" && (
                <>
                  <SharingWorkerStatus revision={state.saved_revision} />
                  <h3>Saved sharing configuration</h3>
                  <p>These are saved worker settings, not a live sharing status report.</p>
                </>
              )}
              {state && readOnly && backendSection && (
                <SectionSummary configuration={state.saved} section={backendSection} />
              )}
              {state && editor && !readOnly && backendSection && (
                <BackendConfigurationForm
                  editor={editor}
                  saved={state.saved}
                  section={backendSection}
                  changed={changed.includes(backendSection)}
                  disabled={command.busy || !fresh || readOnly || reloading || uncertain}
                  busy={command.busy}
                  onChange={(draft) => setEditor(changeConfigurationDraft(editor, draft))}
                  onValidate={() => void submit("validate", [backendSection])}
                  onSave={() => void submit("save", [backendSection])}
                  onSaveAll={changed.length > 1 ? () => void submit("save", changed) : undefined}
                  onDiscard={() =>
                    setEditor(
                      editor.conflict
                        ? undefined
                        : discardConfigurationSection(editor, backendSection),
                    )
                  }
                />
              )}
              {state && backendSection && backendSection === "music_providers" && (
                <ArtistPhotoBindings
                  readOnly={readOnly || !fresh || reloading || uncertain}
                  enabled={state.active.services.artist_images.includes("wikimedia-commons")}
                />
              )}
              {state && backendSection && (
                <details className="configuration-details">
                  <summary>Advanced</summary>
                  {editor && (
                    <Button
                      variant="secondary"
                      disabled={
                        command.busy ||
                        !fresh ||
                        readOnly ||
                        reloading ||
                        uncertain ||
                        editor.conflict
                      }
                      onClick={() => void submit("validate", [backendSection])}
                    >
                      Check changes before saving
                    </Button>
                  )}
                  {state.saved_requires_validation && (
                    <p>Saved settings need full validation when the API reloads.</p>
                  )}
                  <h3>
                    {backendSection === "music_sharing"
                      ? "API configuration snapshot"
                      : "Currently running"}
                  </h3>
                  {backendSection === "music_sharing" && (
                    <p>The API snapshot does not report the separate worker configuration.</p>
                  )}
                  <SectionSummary configuration={state.active} section={backendSection} />
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
              )}
            </ConfigurationCard>
          </div>
          <div hidden={section !== "music_output"}>{output}</div>
          <div hidden={section !== "music_roots"}>{roots}</div>
        </div>
      </div>
    </RefreshContext.Provider>
  );
}

export function sharingWorkerEvidence(
  worker: WorkerStatus | undefined,
  revision: string,
  fresh: boolean,
): string {
  if (!fresh) return "Worker sharing status is unknown. Refresh the status before relying on it.";
  if (!worker)
    return "Worker sharing status is unknown. The API has no worker status snapshot. Reload the API if its worker settings changed.";
  if (!worker.online)
    return "The worker is offline or its heartbeat is stale. Saved sharing settings are not confirmed active.";
  if (worker.stopping)
    return "The worker is stopping. Saved sharing settings are not confirmed active.";
  if (worker.active_revision !== revision)
    return "The worker runs a different configuration revision. Restart it at a safe boundary to apply the saved sharing settings.";
  return "The worker heartbeat confirms this saved configuration revision. This does not prove that peers can connect or that every folder is available.";
}

export function ManagedCertificateStatus({ revision }: { revision: string }) {
  const resource = useResource(
    (signal) => request<BackendCertificateStatus>("/api/system/certificates/status", { signal }),
    `managed-certificate:${revision}`,
    15000,
  );
  const fresh =
    !resource.loading && !resource.error && resource.data?.configuration_revision === revision;
  const value = fresh ? resource.data?.certificate : undefined;
  const time = (seconds: number | null) => {
    if (seconds === null) return "Not recorded";
    const date = new Date(seconds * 1000);
    return Number.isSafeInteger(seconds) && !Number.isNaN(date.getTime())
      ? date.toLocaleString()
      : "Invalid timestamp. Use local recovery.";
  };
  return (
    <div>
      <h3>Active certificate status</h3>
      {!value ? (
        <p role="status">Certificate status is unknown. Refresh before relying on it.</p>
      ) : (
        <>
          <dl className="properties">
            <dt>Lifecycle phase</dt>
            <dd>{value.phase.replaceAll("_", " ")}</dd>
            <dt>Certificate expiry</dt>
            <dd>{time(value.expires_at)}</dd>
            <dt>Renewal time</dt>
            <dd>{time(value.renew_at)}</dd>
            <dt>Next attempt</dt>
            <dd>{time(value.next_attempt)}</dd>
            <dt>Last success</dt>
            <dd>{time(value.last_success)}</dd>
          </dl>
          {value.expires_at !== null && value.expires_at * 1000 <= Date.now() && (
            <p role="alert">The active certificate has expired. Use local recovery.</p>
          )}
          {value.recovery_required && <p role="alert">Local certificate recovery is required.</p>}
          {value.cleanup_pending && (
            <p role="status">DNS cleanup is pending. The recorded certificate is retained.</p>
          )}
        </>
      )}
      <Alert error={resource.error} />
      <Button variant="ghost" disabled={resource.loading} onClick={resource.refresh}>
        Refresh certificate status
      </Button>
    </div>
  );
}

function SharingWorkerStatus({ revision }: { revision: string }) {
  const status = useResource(
    (signal) => request<LocalStatus>("/api/local/status", { signal }),
    `sharing-worker:${revision}`,
    5000,
  );
  const fresh = !status.loading && !status.error && status.data !== undefined;
  return (
    <div>
      <p role="status">{sharingWorkerEvidence(status.data?.worker, revision, fresh)}</p>
      {fresh && status.data?.worker?.active_revision && (
        <p>
          Worker revision: <code>{status.data.worker.active_revision}</code>.
        </p>
      )}
      <Alert error={status.error} />
      <Button variant="ghost" disabled={status.loading} onClick={status.refresh}>
        Refresh worker status
      </Button>
    </div>
  );
}

function SectionSummary({
  configuration: value,
  section,
}: {
  configuration: BackendConfigurationState["active"];
  section: ConfigurationSection;
}) {
  const rows: [string, ReactNode][] =
    section === "general"
      ? [
          [
            "Access mode",
            value.environment === "single_user"
              ? "Single user · anonymous HTTP"
              : value.environment === "development"
                ? "Development · anonymous HTTPS"
                : "Production · Entra HTTPS",
          ],
          ["Listen address", value.listen],
          ["Browser origin", value.origin],
          ["Browser assets", value.browser_assets ?? "Not configured"],
        ]
      : section === "identity"
        ? value.environment === "single_user"
          ? [["Authentication", "Disabled. Saved Entra references are inactive."]]
          : [
              ["Entra tenant", configured(value.tenant_id_configured)],
              ["Authorized Entra object", configured(value.object_id_configured)],
              ["Entra client", configured(value.client_id_configured)],
              ["Entra secret file", configured(value.client_secret_file_configured)],
            ]
        : section === "tls"
          ? value.environment === "single_user"
            ? [["TLS", "Not enabled. Single-user HTTP needs no certificate or Azure setup."]]
            : value.tls_mode === "azure_dns"
              ? [
                  ["Certificate source", "Let's Encrypt with Azure DNS"],
                  ["ACME environment", value.managed_tls?.environment ?? "Not configured"],
                  ["Certificate state", value.managed_tls?.state_directory ?? "Not configured"],
                  ["Azure DNS zone", value.managed_tls?.zone ?? "Not configured"],
                  ["DNS tenant", configured(value.managed_tls?.tenant_id_configured ?? false)],
                  ["DNS client", configured(value.managed_tls?.client_id_configured ?? false)],
                  [
                    "DNS secret file",
                    configured(value.managed_tls?.client_secret_file_configured ?? false),
                  ],
                ]
              : [
                  ["TLS certificate file", value.certificate_file ?? "Not configured"],
                  ["TLS private-key file", configured(value.private_key_file_configured)],
                ]
          : section === "storage"
            ? [
                ["Local database", value.database],
                [
                  "Provider cache",
                  value.cache ? "Shared private file cache" : "Legacy SQLite cache",
                ],
                ...(value.cache
                  ? ([
                      ["Cache root", value.cache.root],
                      ["Maximum cache item", `${value.cache.item_bytes} bytes`],
                      ["Maximum cache storage", `${value.cache.total_bytes} bytes`],
                      ["Maximum cache entries", value.cache.entries],
                    ] satisfies [string, ReactNode][])
                  : []),
              ]
            : section === "music_providers"
              ? [
                  ["Discovery providers", value.services.discovery.join(", ") || "Disabled"],
                  ["Metadata providers", value.services.metadata.join(", ") || "Disabled"],
                  ["Artwork providers", value.services.artwork.join(", ") || "Disabled"],
                  ["Acquisition providers", value.services.acquisition.join(", ") || "Disabled"],
                  ["Artist photo source", value.services.artist_images.join(", ") || "Disabled"],
                ]
              : section === "music_sharing"
                ? [
                    ["Access", value.production_worker?.sharing?.policy.access ?? "none"],
                    ["Shared folders", value.production_worker?.sharing?.policy.roots.length ?? 0],
                    [
                      "Allowlist usernames",
                      value.production_worker?.sharing?.policy.allowlist.join(", ") || "None",
                    ],
                    ["Peer listener", value.production_worker?.sharing?.listen ?? "Not configured"],
                    [
                      "Account",
                      value.production_worker?.sharing?.account
                        ? configured(
                            value.production_worker.sharing.account.credentials_file_configured,
                          )
                        : "Use production acquisition account",
                    ],
                    ["Activation", "Restart the separate worker to apply saved changes."],
                    ...(value.production_worker?.sharing?.policy.roots.map(
                      (root) =>
                        [`Public alias · binding ${root.binding_id}`, root.alias] satisfies [
                          string,
                          ReactNode,
                        ],
                    ) ?? []),
                  ]
                : [
                    [
                      "Development acquisition",
                      value.local_acquisition ? "Configured · development only" : "Disabled",
                    ],
                    ...(value.local_acquisition
                      ? ([
                          ["Private staging root", value.local_acquisition.staging_root],
                          [
                            "Soulseek credential file",
                            configured(value.local_acquisition.credentials_file_configured),
                          ],
                          ["Soulseek server", value.local_acquisition.server],
                          [
                            "Plaintext login acknowledgement",
                            value.local_acquisition.acknowledge_plaintext_login
                              ? "Acknowledged"
                              : "Not acknowledged",
                          ],
                        ] satisfies [string, ReactNode][])
                      : []),
                  ];
  return (
    <dl className="properties">
      {rows.map(([name, content]) => (
        <div className="configuration-property" key={name}>
          <dt>{name}</dt>
          <dd>{content}</dd>
        </div>
      ))}
    </dl>
  );
}
