# Optional Azure DNS setup

Anonymous `domus serve` needs no infrastructure setup. Manual HTTPS needs no
Azure subscription, DNS automation identity, Bicep or Azure CLI. These templates
prepare optional DNS-01 permissions for one existing public Azure DNS zone.
They do not deploy the application, compute, a DNS zone or a resource group.

The layout follows PlagueHO/marginalia. The entry point uses native
`.bicepparam` inputs, a separate Microsoft Graph identity module and pinned
Azure Verified Modules (AVM). AVM telemetry is disabled.
The custom role uses a native subscription-scoped definition because the
inspected AVM role-definition pattern requires management-group access.

## Inputs and scope

Set `DOMUS_DNS_RESOURCE_GROUP`, `DOMUS_DNS_ZONE`, `DOMUS_DNS_HOSTNAME` and
`DOMUS_ENVIRONMENT` before building `main.bicepparam`. The hostname is relative
to the zone. For `api.home.example` in `home.example`, use `api`. Use an empty
hostname for the zone apex. Do not use the ASUS DDNS CNAME target.

Normal deployment creates a dedicated application and service principal, a
custom role and one exact-record role assignment. The role permits TXT
`read` and `write`, not `delete`. The assignment scope is only
`_acme-challenge.<hostname>` in the existing zone. Redeployment does not write
that record or change the CNAME, name servers, unrelated records or zone.

## Record initialization

The exact TXT resource must exist before assigning its permission.
If it already exists, retain its TTL, metadata and every TXT value. Do not run
the initialization template against it.

`initialize-record.bicep` is a separate, one-time operation. Its
`confirmedAbsent` input defaults to `false`, so it does nothing by default.
Enable it only after an operator checks that the exact TXT resource is absent.
Bicep cannot enforce an ETag-safe create. A check made before deployment can
become stale. Use a maintenance window that excludes other DNS writers or an
operator-approved ARM create with `If-None-Match: *`. Never use the template to
reset, empty or repair an existing record.

Initialize an absent record before running `main.bicep`. Keep the initialized
resource after challenge cleanup so its record-scoped role assignment remains
valid.

## Identity and credentials

Microsoft Graph Bicep uses the pinned preview extension. Graph identity changes
need separate tenant permission and operator approval. Graph resources do not
support `what-if`; an ARM what-if result does not prove Graph changes.

Graph Bicep cannot create application passwords. Create a credential separately
for the dedicated DNS application through an approved Entra operator procedure.
Save its value only in a private local secret file. Do not put secrets in Bicep
parameters, outputs, command arguments, URLs, logs or Git. Keep the DNS identity
separate from browser sign-in. No ambient Azure credential chain is required.

The outputs contain only tenant ID, client ID and the challenge resource ID.
They do not contain a password, token or private key.

## Local compilation

Standalone Bicep can compile the templates without Azure CLI or a deployment:

```powershell
bicep build infra\main.bicep --outfile target\infra-main.json
bicep build-params infra\main.bicepparam --outfile target\infra-main.parameters.json
bicep build infra\initialize-record.bicep --outfile target\infra-initialize.json
bicep build-params infra\initialize-record.bicepparam --outfile target\infra-initialize.parameters.json
node --test infra\template.test.mjs
```

The template tests compile both templates and parameter files with isolated
fixture inputs. They check permitted actions, exact assignment scope, identity
settings, disabled initialization, output safety and stable compiled names.
They require the standalone Bicep CLI and Node. They do not use Azure credentials.

Compilation is not a deployment or a permission test. Live deployment, identity
and DNS changes, certificate issuance and public exposure need separate approval.
The application implements managed issuance, renewal and host activation.
Local protocol fixtures are separate from live Azure and certificate-authority
acceptance. Do not treat template compilation as proof of runtime behavior.

## Sources

- [Microsoft Graph Bicep limitations](https://learn.microsoft.com/graph/templates/bicep/limitations)
- [Azure DNS record-set REST API](https://learn.microsoft.com/rest/api/dns/record-sets/create-or-update)
- [Azure Verified Modules](https://azure.github.io/Azure-Verified-Modules/)
- [Bicep parameter files](https://learn.microsoft.com/azure/azure-resource-manager/bicep/parameter-files)
