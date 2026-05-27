#!/usr/bin/env sh
set -eu
ITERATIONS="${1:-24}"
exec aggora-node sim --iterations "$ITERATIONS" --config "${AGGORA_COIN_CONFIG:-config/default.toml}"
