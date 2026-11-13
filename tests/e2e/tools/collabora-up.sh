#!/bin/sh
# Starts Collabora Online (the official collabora/code image, podman) on 127.0.0.1:9980 for
# editing drive files in the browser. NERP must run with COLLABORA_URL=http://127.0.0.1:9980;
# Collabora fetches and saves files through NERP's WOPI endpoints at $NERP_URL.
#   NERP_URL   where Collabora reaches NERP (default http://localhost:8080)
# It shares the host's network so both sides use the same addresses. SSL is off: local use only.
# Uses podman, or docker where podman isn't installed (CI). Stop it with: podman rm -f nerp-collabora
set -e
if command -v podman > /dev/null; then C=podman; else C=docker; fi
NERP_URL="${NERP_URL:-http://localhost:8080}"
IMAGE=docker.io/collabora/code:latest
# The NERP address, and the other spelling of a local one as an alias (aliases are patterns).
ALT="$(printf '%s' "$NERP_URL" | sed 's/localhost/127\\.0\\.0\\.1/')"
if ! $C container inspect nerp-collabora > /dev/null 2>&1; then
  $C run -d --name nerp-collabora --network host \
    -e "aliasgroup1=$NERP_URL,$ALT" \
    -e "extra_params=--o:ssl.enable=false --o:ssl.termination=false --o:net.listen=loopback --o:net.proto=IPv4 --o:welcome.enable=false --o:home_mode.enable=true" \
    -e "DONT_GEN_SSL_CERT=1" \
    "$IMAGE" > /dev/null
fi
$C start nerp-collabora > /dev/null
for _ in $(seq 1 120); do
  curl -sf http://127.0.0.1:9980/hosting/discovery > /dev/null && { echo "Collabora is up on http://127.0.0.1:9980 (WOPI host $NERP_URL)"; exit 0; }
  sleep 1
done
echo "Collabora did not come up; see: $C logs nerp-collabora" >&2
exit 1
