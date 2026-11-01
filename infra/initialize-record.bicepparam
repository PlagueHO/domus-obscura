using './initialize-record.bicep'

param dnsZone = readEnvironmentVariable('DOMUS_DNS_ZONE', 'example.com')
param hostname = readEnvironmentVariable('DOMUS_DNS_HOSTNAME', 'domus')
param confirmedAbsent = false
