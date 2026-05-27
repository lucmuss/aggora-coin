#!/usr/bin/env sh
set -eu
BASE_URL="${BASE_URL:-http://127.0.0.1:18081}"
curl -fsS "$BASE_URL/healthz" >/dev/null
curl -fsS "$BASE_URL/api/v1/stats" | grep -q '"success":true'
echo "aggora-coin smoke test passed at $BASE_URL"
