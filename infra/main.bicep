targetScope = 'subscription'

@sys.description('Existing resource group containing the public DNS zone.')
@minLength(1)
@maxLength(90)
param dnsResourceGroup string

@sys.description('Existing public DNS zone. No zone or name-server changes are made.')
@minLength(1)
@maxLength(253)
param dnsZone string

@sys.description('Relative hostname in that zone, without a trailing dot. Empty selects the zone apex.')
@maxLength(253)
param hostname string

@sys.description('Display-name prefix for the dedicated DNS automation identity.')
@minLength(1)
@maxLength(64)
param environmentName string = 'domus'

resource group 'Microsoft.Resources/resourceGroups@2024-03-01' existing = {
  name: dnsResourceGroup
}

resource zone 'Microsoft.Network/dnsZones@2018-05-01' existing = {
  name: dnsZone
  scope: resourceGroup(dnsResourceGroup)
}

resource challenge 'Microsoft.Network/dnsZones/TXT@2018-05-01' existing = {
  parent: zone
  name: empty(hostname) ? '_acme-challenge' : '_acme-challenge.${hostname}'
}

module identity './entra-id/dns-application.bicep' = {
  name: 'dns-identity-${uniqueString(subscription().id, challenge.id)}'
  scope: resourceGroup(dnsResourceGroup)
  params: {
    uniqueName: '${environmentName}-dns-${uniqueString(tenant().tenantId, challenge.id)}'
  }
}

module role './core/security/dns-role.bicep' = {
  name: 'dns-role-${uniqueString(challenge.id)}'
  params: {
    resourceGroupId: group.id
    challengeResourceId: challenge.id
  }
}

module assignment 'br/public:avm/ptn/authorization/resource-role-assignment:0.1.2' = {
  name: 'dns-permission-${uniqueString(challenge.id)}'
  scope: resourceGroup(dnsResourceGroup)
  params: {
    resourceId: challenge.id
    roleDefinitionId: role.outputs.resourceId
    principalId: identity.outputs.principalId
    principalType: 'ServicePrincipal'
    description: 'Domus ACME automation for one TXT record set only.'
    enableTelemetry: false
  }
}

@sys.description('Tenant used for the automation identity.')
output tenantId string = tenant().tenantId

@sys.description('Client ID; not a credential.')
output clientId string = identity.outputs.clientId

@sys.description('Existing challenge record scope; its values were not changed.')
output challengeResourceId string = challenge.id
