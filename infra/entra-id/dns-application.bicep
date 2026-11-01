extension microsoftGraphV1

@sys.description('Stable application unique name. Redeployment reuses this identity.')
@minLength(1)
@maxLength(128)
param uniqueName string

resource application 'Microsoft.Graph/applications@v1.0' = {
  uniqueName: uniqueName
  displayName: uniqueName
  signInAudience: 'AzureADMyOrg'
}

resource servicePrincipal 'Microsoft.Graph/servicePrincipals@v1.0' = {
  appId: application.appId
}

@sys.description('Application client ID; no password is returned.')
output clientId string = application.appId

@sys.description('Service principal object ID for the record-scoped role.')
output principalId string = servicePrincipal.id
