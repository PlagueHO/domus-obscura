import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const directory = dirname(fileURLToPath(import.meta.url));
const executable = process.platform === "win32" ? "bicep.exe" : "bicep";
const environment = {
  ...process.env,
  DOMUS_DNS_RESOURCE_GROUP: "fixture-dns",
  DOMUS_DNS_ZONE: "home.example",
  DOMUS_DNS_HOSTNAME: "api",
  DOMUS_ENVIRONMENT: "fixture",
};

function compile(operation, file) {
  return JSON.parse(
    execFileSync(executable, [operation, join(directory, file), "--stdout"], {
      encoding: "utf8",
      env: environment,
      maxBuffer: 8 * 1024 * 1024,
      timeout: 120_000,
    }),
  );
}

function resources(value, result = []) {
  if (!value || typeof value !== "object") return result;
  if (value.resources) {
    result.push(...Object.values(value.resources));
  }
  for (const child of Object.values(value)) resources(child, result);
  return result;
}

function parameters(output) {
  return typeof output.parametersJson === "string"
    ? JSON.parse(output.parametersJson).parameters
    : output.parameters;
}

const main = compile("build", "main.bicep");
const initialization = compile("build", "initialize-record.bicep");
const mainParameters = parameters(compile("build-params", "main.bicepparam"));
const initializationParameters = parameters(
  compile("build-params", "initialize-record.bicepparam"),
);
const deployed = resources(main);

test("normal setup has no DNS, resource-group, compute or secret writes", () => {
  const allowed = new Set([
    "Microsoft.Resources/deployments",
    "Microsoft.Authorization/roleDefinitions",
    "Microsoft.Authorization/roleAssignments",
    "Microsoft.Graph/applications@v1.0",
    "Microsoft.Graph/servicePrincipals@v1.0",
  ]);
  assert.ok(deployed.length > 0);
  for (const resource of deployed) {
    assert.ok(allowed.has(resource.type), `Unexpected resource: ${resource.type}`);
  }
  assert.deepEqual(Object.keys(main.outputs).sort(), [
    "challengeResourceId",
    "clientId",
    "tenantId",
  ]);
  assert.doesNotMatch(
    JSON.stringify(main),
    /passwordCredentials|keyCredentials|clientSecret|redirectUris/,
  );
});

test("the custom role has exactly TXT read and write permissions", () => {
  const roles = deployed.filter(
    (resource) => resource.type === "Microsoft.Authorization/roleDefinitions",
  );
  assert.equal(roles.length, 1);
  const role = roles[0];
  assert.equal(role.properties.type, "CustomRole");
  assert.deepEqual(role.properties.assignableScopes, ["[parameters('resourceGroupId')]"]);
  assert.deepEqual(role.properties.permissions, [
    {
      actions: ["Microsoft.Network/dnsZones/TXT/read", "Microsoft.Network/dnsZones/TXT/write"],
      notActions: [],
      dataActions: [],
      notDataActions: [],
    },
  ]);
  assert.equal(role.name, "[guid(parameters('challengeResourceId'), 'domus-acme-txt')]");
});

test("AVM assigns the role to the exact existing challenge resource", () => {
  const modules = Object.values(main.resources);
  const assignment = modules.find((resource) => resource.properties.parameters.resourceId);
  assert.ok(assignment);
  const inputs = assignment.properties.parameters;
  assert.equal(inputs.resourceId.value, main.outputs.challengeResourceId.value);
  assert.match(inputs.resourceId.value, /'Microsoft.Network\/dnsZones\/TXT'/);
  assert.match(inputs.resourceId.value, /parameters\('dnsZone'\)/);
  assert.match(inputs.resourceId.value, /'_acme-challenge'/);
  assert.match(inputs.resourceId.value, /'_acme-challenge\.\{0\}'/);
  assert.equal(inputs.principalType.value, "ServicePrincipal");
  assert.equal(inputs.enableTelemetry.value, false);

  const inner = resources(assignment.properties.template);
  assert.ok(
    inner.some(
      (resource) => resource.properties?.parameters?.scope?.value === "[parameters('resourceId')]",
    ),
  );
  const assignments = inner.filter(
    (resource) => resource.type === "Microsoft.Authorization/roleAssignments",
  );
  assert.equal(assignments.length, 1);
  // AVM stores this inner template in an outer ARM variable.
  assert.equal(assignments[0].scope, "[[parameters('scope')]");
  assert.equal(assignments[0].properties.principalType, "[[parameters('principalType')]");
});

test("Graph identity is single-tenant and has no sign-in or delegated permissions", () => {
  const applications = deployed.filter(
    (resource) => resource.type === "Microsoft.Graph/applications@v1.0",
  );
  assert.equal(applications.length, 1);
  assert.deepEqual(applications[0].properties, {
    uniqueName: "[parameters('uniqueName')]",
    displayName: "[parameters('uniqueName')]",
    signInAudience: "AzureADMyOrg",
  });
  assert.equal(
    deployed.filter((resource) => resource.type === "Microsoft.Graph/servicePrincipals@v1.0")
      .length,
    1,
  );
});

test("record initialization remains separate and disabled by default", () => {
  assert.equal(initialization.parameters.confirmedAbsent.defaultValue, false);
  assert.equal(initializationParameters.confirmedAbsent.value, false);
  const modules = Object.values(initialization.resources);
  assert.equal(modules.length, 1);
  assert.equal(modules[0].condition, "[parameters('confirmedAbsent')]");
  assert.equal(modules[0].properties.parameters.enableTelemetry.value, false);
  assert.deepEqual(modules[0].properties.parameters.txtRecords.value, []);
  assert.deepEqual(modules[0].properties.parameters.metadata.value, {
    owner: "domus-setup",
  });
  assert.ok(
    resources(initialization).some(
      (resource) => resource.type === "Microsoft.Network/dnsZones/TXT",
    ),
  );
});

test("parameter files compile with isolated non-secret fixture inputs", () => {
  assert.deepEqual(mainParameters, {
    dnsResourceGroup: { value: "fixture-dns" },
    dnsZone: { value: "home.example" },
    hostname: { value: "api" },
    environmentName: { value: "fixture" },
  });
  assert.equal(initializationParameters.dnsZone.value, "home.example");
  assert.equal(initializationParameters.hostname.value, "api");
});

test("repeat compilation preserves identity, role and assignment names", () => {
  assert.deepEqual(compile("build", "main.bicep"), main);
});
