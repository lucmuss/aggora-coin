# Runbook

## Health

```sh
curl -fsS http://127.0.0.1:18081/healthz
curl -fsS http://127.0.0.1:18081/api/v1/stats
```

## Rotate Development Data

```sh
docker compose down -v
docker compose up --build
```

## Generate Operator Key

```sh
docker compose run --rm aggora-coin aggora-node keygen
```

Set the generated `secret` as `AGORA_OPERATOR_SECRET`. The public key and operator id are derived automatically at boot.

## Manual Iteration

Use a signed admin request against `POST /admin/simulation/start` with body `{"iterations":1}`. The endpoint executes the same iteration engine used by the simulator and writes a snapshot when enabled.
