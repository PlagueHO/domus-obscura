using './main.bicep'

param dnsResourceGroup = readEnvironmentVariable('DOMUS_DNS_RESOURCE_GROUP', 'replace-with-existing-resource-group')
param dnsZone = readEnvironmentVariable('DOMUS_DNS_ZONE', 'example.com')
param hostname = readEnvironmentVariable('DOMUS_DNS_HOSTNAME', 'domus')
param environmentName = readEnvironmentVariable('DOMUS_ENVIRONMENT', 'domus')
