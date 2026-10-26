import type { BackendConfigurationDraft } from "../src/generated/BackendConfigurationDraft";
import type { BackendConfigurationIssue } from "../src/generated/BackendConfigurationIssue";
import type { BackendConfigurationState } from "../src/generated/BackendConfigurationState";

export type ConfigurationEditor = {
  draft: BackendConfigurationDraft;
  expectedRevision: string;
  errors: BackendConfigurationIssue[];
  validated: boolean;
  conflict: boolean;
};

export function beginConfigurationEdit(state: BackendConfigurationState): ConfigurationEditor {
  const value = state.saved;
  return {
    expectedRevision: state.saved_revision,
    errors: [],
    validated: false,
    conflict: false,
    draft: {
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
    },
  };
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
