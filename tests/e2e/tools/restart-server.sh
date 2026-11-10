#!/bin/sh
# Restarts the dev server (backend/target/debug/nerp) with the environment the full test run
# needs (see ../README.md), logging to $NERP_E2E_OUTPUT/server.log or /tmp/nerp-e2e/server.log.
cd "$(dirname "$0")/../../../backend" || exit 1
OUT="${NERP_E2E_OUTPUT:-${TMPDIR:-/tmp}/nerp-e2e}"
mkdir -p "$OUT/livekit-recordings"
pkill -f "target/debug/nerp$" && sleep 2
STATIC_DIR=../frontend/build CRM_WEBHOOK_ALLOW_PRIVATE=true LDAP_ENABLED=true PUBLIC_URL=http://localhost:8080 \
WEBRTC_ICE_SERVERS='[{"urls":"turn:127.0.0.1:3478?transport=udp","username":"nerp-test","credential":"nerp-test-turn"}]' \
LIVEKIT_URL=ws://127.0.0.1:7890 LIVEKIT_API_URL=http://127.0.0.1:7899 LIVEKIT_API_KEY=nerp-test-key \
LIVEKIT_API_SECRET=nerp-test-secret-0123456789abcdefghijkl LIVEKIT_RECORDINGS_DIR="$OUT/livekit-recordings" \
setsid nohup ./target/debug/nerp > "$OUT/server.log" 2>&1 < /dev/null &
for _ in $(seq 1 90); do curl -sf http://localhost:8080/api/health > /dev/null && exit 0; sleep 1; done
echo "the server did not come up; see $OUT/server.log" >&2
exit 1
