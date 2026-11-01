@sys.description('Existing DNS zone. Run only after confirming that the TXT record does not exist.')
@minLength(1)
@maxLength(253)
param dnsZone string

@sys.description('Relative hostname in the existing zone. Empty selects the zone apex.')
@maxLength(253)
param hostname string

@sys.description('Explicit operator confirmation of absence. This template is never part of normal redeployment.')
param confirmedAbsent bool = false

module record 'br/public:avm/res/network/dns-zone/txt:0.1.0' = if (confirmedAbsent) {
  name: 'domus-initialize-challenge'
  params: {
    dnsZoneName: dnsZone
    name: empty(hostname) ? '_acme-challenge' : '_acme-challenge.${hostname}'
    txtRecords: []
    ttl: 60
    metadata: {
      owner: 'domus-setup'
    }
    enableTelemetry: false
  }
}
