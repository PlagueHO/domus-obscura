import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { BackendCertificateStatus } from "../src/generated/BackendCertificateStatus";
import type { ConfigurationSection } from "../src/generated/ConfigurationSection";
import type { WorkerStatus } from "../src/generated/WorkerStatus";
import type { LocalStatus } from "../src/generated/LocalStatus";
import { request } from "./api";
import { useResource } from "./hooks";
import { Alert, Button, HeaderIconAction, HeaderCard, SettingsSaveTarget } from "./components/ui";
import { Icon } from "./components/Media";
import { ArtistPhotoBindings } from "./ArtistPhotoBindings";
import { SoulseekAccount } from "./SoulseekAccount";
import { configurationSections } from "./backend-configuration-state";

type Section = ConfigurationSection | "music_output" | "music_roots";
const configured = (value: boolean) => (value ? "Configured · value hidden" : "Not configured");
const RefreshContext = createContext<{ busy: boolean; refresh: () => void } | undefined>(undefined);

export function ConfigurationCard({ title, children }: { title: string; children: ReactNode }) {
  const refresh = useContext(RefreshContext);
  return (
    <HeaderCard
      title={title}
      className="configuration-section"
      actions={
        refresh && (
          <HeaderIconAction
            label="Refresh settings"
            disabled={refresh.busy}
            onClick={refresh.refresh}
          >
            <Icon name="refresh" />
          </HeaderIconAction>
        )
      }
    >
      {children}
    </HeaderCard>
  );
}

export function BackendConfiguration({
  output,
  roots,
  onRefreshRelated,
  relatedLoading = false,
}: {
  output?: ReactNode;
  roots?: ReactNode;
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
    if (requested === "music_soulseek") return "music_acquisition";
    return requested === "music_roots" || requested === "music_output"
      ? requested
      : (configurationSections.find(({ id }) => id === requested)?.id ?? "general");
  });
  const content = useRef<HTMLDivElement>(null);
  const [saveTarget, setSaveTarget] = useState<HTMLDivElement | null>(null);
  useEffect(() => {
    content.current?.scrollTo(0, 0);
  }, [section]);
  const state = resource.data;
  const fresh = !resource.error && !resource.loading;
  const policyReadOnly =
    !state || state.active.system_settings_read_only || state.saved.system_settings_read_only;
  const currentSection = configurationSections.find(({ id }) => id === section);
  const backendSection = currentSection?.id;
  const select = (value: string) => {
    const target = configurationSections.find(({ id }) => id === value);
    if (target) setSection(target.id);
    else if (value === "music_output" || value === "music_roots") setSection(value);
  };
  const navigation = (id: Section, label: string) => (
    <Button
      key={id}
      variant="ghost"
      type="button"
      aria-current={section === id ? "page" : undefined}
      onClick={() => setSection(id)}
    >
      <span>{label}</span>
      {state?.changed_sections.some((value) => value === id) && <small>Restart required</small>}
    </Button>
  );
  return (
    <RefreshContext.Provider
      value={{
        busy: resource.loading || relatedLoading,
        refresh: () => {
          resource.refresh();
          onRefreshRelated?.();
        },
      }}
    >
      <div className="configuration-layout">
        <label className="configuration-section-picker">
          Settings section
          <select value={section} onChange={(event) => select(event.target.value)}>
            {["Core application", "Media · Music"].map((group) => (
              <optgroup key={group} label={group}>
                {configurationSections
                  .filter((value) => value.group === group)
                  .map(({ id, title }) => (
                    <option key={id} value={id}>
                      {title}
                    </option>
                  ))}
                {group === "Media · Music" && output && (
                  <option value="music_output">Output policy</option>
                )}
                {group === "Media · Music" && roots && (
                  <option value="music_roots">Library folders</option>
                )}
              </optgroup>
            ))}
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
              .map(({ id, title }) => navigation(id, title))}
          </div>
          <div className="configuration-group">
            <div className="configuration-group-label">
              <Icon name="music" />
              Media
            </div>
            <div className="configuration-subgroup">Music</div>
            <div className="configuration-subgroup-items">
              {configurationSections
                .filter(({ group }) => group === "Media · Music")
                .map(({ id, title }) => navigation(id, title))}
              {output && navigation("music_output", "Output policy")}
              {roots && navigation("music_roots", "Library folders")}
            </div>
          </div>
        </nav>
        <div className="configuration-pane">
          <SettingsSaveTarget.Provider value={saveTarget}>
            <div
              ref={content}
              className="configuration-content"
              role="region"
              aria-label="Settings content"
              tabIndex={0}
            >
              <div className="configuration-messages">
                <Alert error={resource.error} />
                <p className="muted">
                  Primary library and Soulseek account changes restart the API and worker. Other
                  host settings are managed by the operator.
                </p>
                {state?.restart_required && (
                  <div className="configuration-notice" role="status">
                    <div>
                      <strong>Saved host settings are not active</strong>
                      <p>
                        A coordinated restart is required. Refresh only reads the file; it does not
                        apply changes.
                      </p>
                    </div>
                  </div>
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
                  {state && backendSection && (
                    <SectionSummary configuration={state.saved} section={backendSection} />
                  )}
                  {state && backendSection === "music_providers" && (
                    <ArtistPhotoBindings
                      readOnly={policyReadOnly || !fresh}
                      enabled={state.active.services.artist_images.includes("wikimedia-commons")}
                    />
                  )}
                  {state && backendSection && (
                    <details className="configuration-details">
                      <summary>Advanced</summary>
                      {state.saved_requires_validation && (
                        <p>Saved settings need full validation before all components restart.</p>
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
              {backendSection === "music_acquisition" && (
                <ConfigurationCard title="Soulseek account">
                  <SoulseekAccount
                    disabled={policyReadOnly || !fresh}
                    onChanged={resource.refresh}
                  />
                </ConfigurationCard>
              )}
              <div hidden={section !== "music_output"}>
                <SettingsSaveTarget.Provider value={section === "music_output" ? saveTarget : null}>
                  {output}
                </SettingsSaveTarget.Provider>
              </div>
              <div hidden={section !== "music_roots"}>
                <SettingsSaveTarget.Provider value={section === "music_roots" ? saveTarget : null}>
                  {roots}
                </SettingsSaveTarget.Provider>
              </div>
            </div>
          </SettingsSaveTarget.Provider>
          <div
            ref={setSaveTarget}
            className="settings-save-footer"
            role="group"
            aria-label="Settings save actions"
          />
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
  if (!worker) return "Worker sharing status is unknown. The API has no worker status snapshot.";
  if (!worker.online)
    return "The worker is offline or its heartbeat is stale. Saved sharing settings are not confirmed active.";
  if (worker.stopping)
    return "The worker is stopping. Saved sharing settings are not confirmed active.";
  if (worker.active_revision !== revision)
    return "The worker runs a different configuration revision. Stop all components, validate the configuration, then restart them.";
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
  const acquisition = value.production_worker?.acquisition ?? value.local_acquisition;
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
                    [
                      "Activation",
                      "Stop all components, edit and validate the file, then restart them.",
                    ],
                    ...(value.production_worker?.sharing?.policy.roots.map(
                      (root) =>
                        [`Public alias · binding ${root.binding_id}`, root.alias] satisfies [
                          string,
                          ReactNode,
                        ],
                    ) ?? []),
                  ]
                : [
                    ["Production worker", value.production_worker ? "Configured" : "Disabled"],
                    ["Queue capacity", value.production_worker?.queue_capacity ?? "Not configured"],
                    [
                      "Development acquisition",
                      value.local_acquisition ? "Configured · development only" : "Disabled",
                    ],
                    ...(acquisition
                      ? ([
                          ["Private staging root", acquisition.staging_root],
                          [
                            "Soulseek credential file",
                            configured(acquisition.credentials_file_configured),
                          ],
                          ["Soulseek server", acquisition.server],
                          [
                            "Plaintext login acknowledgement",
                            acquisition.acknowledge_plaintext_login
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
