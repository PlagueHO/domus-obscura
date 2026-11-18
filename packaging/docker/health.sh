#!/bin/sh
set -eu
if [ -n "${DOMUS_HEALTH_CA:-}" ]; then
  set -- --cacert "$DOMUS_HEALTH_CA"
else
  set --
fi
status=$(curl --fail --silent --show-error --output /dev/null --write-out '%{http_code}' \
  --noproxy '*' --proto '=http,https' --max-redirs 0 --max-time 3 \
  "$@" "${DOMUS_HEALTH_URL:?Set the configured browser origin}")
if [ "$status" != "200" ]; then
  echo "Browser health probe returned HTTP $status" >&2
  exit 1
fi
