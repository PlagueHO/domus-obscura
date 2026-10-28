import type { BackendConfigurationDraft } from "../src/generated/BackendConfigurationDraft";
import type { BackendConfigurationIssue } from "../src/generated/BackendConfigurationIssue";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { ConfigurationSection } from "../src/generated/ConfigurationSection";
import type { ConfigurationSectionUpdate } from "../src/generated/ConfigurationSectionUpdate";
import type { BackendSharingDraft } from "../src/generated/BackendSharingDraft";

export function newSharingDraft(): BackendSharingDraft {
  return {
    policy: {
      access: "none",
      allowlist: [],
      roots: [],
      limits: {
        upload_slots: 2,
        queued_files: 128,
        queued_per_peer: 16,
        buffer_bytes: 65536,
        roots: 16,
        catalog_files: 1000000,
        catalog_bytes: 268435456,
        expanded_bytes: 268435456,
        compressed_bytes: 67108864,
        spool_bytes: 268435456,
        peer_connections: 16,
        pending_handshakes: 4,
        requests_per_minute: 60,
        search_results: 200,
        concurrent_browses: 2,
        idle_seconds: 10,
        operation_seconds: 1800,
      },
    },
    account: null,
    listen: null,
  };
}

export const configurationSections = [
  {
    id: "general",
    title: "General",
    group: "Core application",
    description: "Set the server address and browser access.",
  },
  {
    id: "identity",
    title: "Identity",
    group: "Core application",
    description: "Manage Microsoft Entra sign-in and administrator access.",
  },
  {
    id: "tls",
    title: "TLS",
    group: "Core application",
    description: "Choose the certificate and private key for HTTPS.",
  },
  {
    id: "storage",
    title: "Storage/cache",
    group: "Core application",
    description: "Choose the local database and provider cache.",
  },
  {
    id: "music_providers",
    title: "Providers",
    group: "Media · Music",
    description: "Choose the services used to find and download music.",
  },
  {
    id: "music_acquisition",
    title: "Acquisition",
    group: "Media · Music",
    description: "Manage the production worker or the development download service.",
  },
  {
    id: "music_sharing",
    title: "Sharing",
    group: "Media · Music",
    description: "Share selected library folders through the separate Soulseek worker.",
  },
] as const satisfies readonly {
  id: ConfigurationSection;
  title: string;
  group: string;
  description: string;
}[];

const sectionFields = {
  general: ["environment", "listen", "origin", "browser_assets"],
  identity: ["tenant_id", "object_id", "client_id", "client_secret_file"],
  tls: ["certificate_file", "private_key_file"],
  storage: ["database", "cache"],
  music_providers: ["services"],
  music_acquisition: ["local_acquisition", "production_worker"],
  music_sharing: ["production_worker"],
} as const satisfies Record<ConfigurationSection, readonly (keyof BackendConfigurationDraft)[]>;

export type ConfigurationEditor = {
  draft: BackendConfigurationDraft;
  baseline: BackendConfigurationDraft;
  expectedRevision: string;
  errors: BackendConfigurationIssue[];
  validated: boolean;
  conflict: boolean;
};

export function beginConfigurationEdit(state: BackendConfigurationState): ConfigurationEditor {
  const value = state.saved;
  const draft: BackendConfigurationDraft = {
    environment: value.environment,
    services: structuredClone(value.services),
    cache: value.cache ? { ...value.cache } : null,
    database: value.database,
    listen: value.listen,
    origin: value.origin,
    tenant_id: { action: "keep" },
    object_id: { action: "keep" },
    client_id: { action: "keep" },
    client_secret_file: { action: "keep" },
    certificate_file: value.certificate_file,
    private_key_file: { action: "keep" },
    browser_assets: value.browser_assets,
    local_acquisition: value.local_acquisition
      ? {
          staging_root: value.local_acquisition.staging_root,
          credentials_file: { action: "keep" },
          server: value.local_acquisition.server,
          acknowledge_plaintext_login: value.local_acquisition.acknowledge_plaintext_login,
        }
      : null,
    production_worker: value.production_worker
      ? {
          queue_capacity: value.production_worker.queue_capacity,
          acquisition: value.production_worker.acquisition
            ? {
                staging_root: value.production_worker.acquisition.staging_root,
                credentials_file: { action: "keep" },
                server: value.production_worker.acquisition.server,
                acknowledge_plaintext_login:
                  value.production_worker.acquisition.acknowledge_plaintext_login,
              }
            : null,
          sharing: value.production_worker.sharing
            ? {
                policy: structuredClone(value.production_worker.sharing.policy),
                listen: value.production_worker.sharing.listen,
                account: value.production_worker.sharing.account
                  ? {
                      credentials_file: { action: "keep" },
                      server: value.production_worker.sharing.account.server,
                      acknowledge_plaintext_login:
                        value.production_worker.sharing.account.acknowledge_plaintext_login,
                    }
                  : null,
              }
            : null,
        }
      : null,
  };
  return {
    expectedRevision: state.saved_revision,
    errors: [],
    validated: false,
    conflict: false,
    draft,
    baseline: structuredClone(draft),
  };
}

export function sectionUpdate(
  draft: BackendConfigurationDraft,
  section: ConfigurationSection,
): ConfigurationSectionUpdate {
  switch (section) {
    case "general":
      return {
        section,
        environment: draft.environment,
        listen: draft.listen,
        origin: draft.origin,
        browser_assets: draft.browser_assets,
      };
    case "identity":
      return {
        section,
        tenant_id: draft.tenant_id,
        object_id: draft.object_id,
        client_id: draft.client_id,
        client_secret_file: draft.client_secret_file,
      };
    case "tls":
      return {
        section,
        certificate_file: draft.certificate_file,
        private_key_file: draft.private_key_file,
      };
    case "storage":
      return { section, database: draft.database, cache: draft.cache };
    case "music_providers":
      return { section, services: draft.services };
    case "music_acquisition":
      return {
        section,
        local_acquisition: draft.local_acquisition,
        production_worker: draft.production_worker
          ? {
              queue_capacity: draft.production_worker.queue_capacity,
              acquisition: draft.production_worker.acquisition,
            }
          : null,
      };
    case "music_sharing":
      return { section, sharing: draft.production_worker?.sharing ?? null };
  }
}

export function dirtySections(editor: ConfigurationEditor): ConfigurationSection[] {
  return configurationSections
    .filter(({ id }) => {
      if (
        id === "music_acquisition" &&
        !editor.baseline.production_worker &&
        editor.draft.production_worker?.sharing &&
        !editor.draft.production_worker.acquisition &&
        editor.draft.production_worker.queue_capacity === 64
      ) {
        return (
          JSON.stringify(editor.draft.local_acquisition) !==
          JSON.stringify(editor.baseline.local_acquisition)
        );
      }
      return (
        JSON.stringify(sectionUpdate(editor.draft, id)) !==
        JSON.stringify(sectionUpdate(editor.baseline, id))
      );
    })
    .map(({ id }) => id);
}

export function sectionHasErrors(
  editor: ConfigurationEditor,
  section: ConfigurationSection,
): boolean {
  return editor.errors.some(({ field }) =>
    section === "music_sharing"
      ? field.startsWith("production_worker.sharing")
      : section === "music_acquisition" && field.startsWith("production_worker.sharing")
        ? false
        : sectionFields[section].some((name) => field === name || field.startsWith(`${name}.`)),
  );
}

export function acceptConfigurationSave(
  editor: ConfigurationEditor,
  state: BackendConfigurationState,
  submitted: ConfigurationSection[],
): ConfigurationEditor {
  const next = beginConfigurationEdit(state);
  for (const section of dirtySections(editor)) {
    if (!submitted.includes(section)) {
      next.draft = copySection(next.draft, editor.draft, section);
    }
  }
  return next;
}

export function discardConfigurationSection(
  editor: ConfigurationEditor,
  section: ConfigurationSection,
): ConfigurationEditor {
  return {
    ...editor,
    draft: copySection(editor.draft, editor.baseline, section),
    errors: [],
    validated: false,
  };
}

function copySection(
  target: BackendConfigurationDraft,
  source: BackendConfigurationDraft,
  section: ConfigurationSection,
): BackendConfigurationDraft {
  if (section === "music_sharing") {
    const sharing = structuredClone(source.production_worker?.sharing ?? null);
    if (
      !source.production_worker &&
      !sharing &&
      target.production_worker?.queue_capacity === 64 &&
      !target.production_worker.acquisition
    ) {
      return { ...target, production_worker: null };
    }
    return {
      ...target,
      production_worker: target.production_worker
        ? { ...target.production_worker, sharing }
        : sharing
          ? { queue_capacity: 64, acquisition: null, sharing }
          : null,
    };
  }
  if (section === "music_acquisition") {
    const sharing = target.production_worker?.sharing ?? null;
    return {
      ...target,
      local_acquisition: structuredClone(source.local_acquisition),
      production_worker: source.production_worker
        ? { ...structuredClone(source.production_worker), sharing }
        : sharing
          ? {
              queue_capacity: target.production_worker?.queue_capacity ?? 64,
              acquisition: null,
              sharing,
            }
          : null,
    };
  }
  const values = Object.fromEntries(
    sectionFields[section].map((field) => [field, structuredClone(source[field])]),
  );
  return { ...target, ...values };
}

export function changeConfigurationDraft(
  editor: ConfigurationEditor,
  draft: BackendConfigurationDraft,
): ConfigurationEditor {
  return { ...editor, draft, validated: false, errors: [] };
}

export function configurationErrors(
  editor: ConfigurationEditor,
  errors: BackendConfigurationIssue[],
): ConfigurationEditor {
  return { ...editor, errors, validated: errors.length === 0 };
}
