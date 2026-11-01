targetScope = 'subscription'

@sys.description('Existing resource group in which this custom role can be assigned.')
param resourceGroupId string

@sys.description('Exact TXT record resource ID used to give this role a stable name.')
param challengeResourceId string

// AVM's role-definition pattern requires management-group scope. A native
// subscription-scoped definition avoids unnecessary management-group access.
resource role 'Microsoft.Authorization/roleDefinitions@2022-04-01' = {
  name: guid(challengeResourceId, 'domus-acme-txt')
  properties: {
    roleName: 'Domus ACME TXT ${uniqueString(challengeResourceId)}'
    description: 'Read and update one existing ACME TXT record set.'
    type: 'CustomRole'
    assignableScopes: [resourceGroupId]
    permissions: [
      {
        actions: [
          'Microsoft.Network/dnsZones/TXT/read'
          'Microsoft.Network/dnsZones/TXT/write'
        ]
        notActions: []
        dataActions: []
        notDataActions: []
      }
    ]
  }
}

@sys.description('Custom role definition ID.')
output resourceId string = role.id
