import type { BackendConfigurationDraft } from "../src/generated/BackendConfigurationDraft";
import type { BackendConfigurationIssue } from "../src/generated/BackendConfigurationIssue";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";
import type { ConfigurationSection } from "../src/generated/ConfigurationSection";
import type { ConfigurationSectionUpdate } from "../src/generated/ConfigurationSectionUpdate";

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
    description: "Manage the optional development download service.",
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
  music_acquisition: ["local_acquisition"],
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
      return { section, local_acquisition: draft.local_acquisition };
  }
}

export function dirtySections(editor: ConfigurationEditor): ConfigurationSection[] {
  return configurationSections
    .filter(
      ({ id }) =>
        JSON.stringify(sectionUpdate(editor.draft, id)) !==
        JSON.stringify(sectionUpdate(editor.baseline, id)),
    )
    .map(({ id }) => id);
}

export function sectionHasErrors(
  editor: ConfigurationEditor,
  section: ConfigurationSection,
): boolean {
  return editor.errors.some(({ field }) =>
    sectionFields[section].some((name) => field === name || field.startsWith(`${name}.`)),
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
      const values = Object.fromEntries(
        sectionFields[section].map((field) => [field, editor.draft[field]]),
      );
      next.draft = { ...next.draft, ...values };
    }
  }
  return next;
}

export function discardConfigurationSection(
  editor: ConfigurationEditor,
  section: ConfigurationSection,
): ConfigurationEditor {
  const values = Object.fromEntries(
    sectionFields[section].map((field) => [field, editor.baseline[field]]),
  );
  return { ...editor, draft: { ...editor.draft, ...values }, errors: [], validated: false };
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
