FROM rust:1-slim-bookworm AS builder
WORKDIR /app
COPY Cargo.toml ./
COPY crates ./crates
RUN cargo build --release -p aggora-cli

FROM debian:bookworm-slim
ENV RUST_LOG=info
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m -d /home/aggora aggora \
    && mkdir -p /app/data /app/snapshots \
    && chown -R aggora:aggora /app
COPY --from=builder /app/target/release/aggora-node /usr/local/bin/aggora-node
COPY config ./config
COPY seeds ./seeds
USER aggora
EXPOSE 8080
VOLUME ["/app/data", "/app/snapshots"]
HEALTHCHECK --interval=10s --timeout=5s --retries=12 --start-period=10s \
  CMD curl -fsS http://127.0.0.1:8080/healthz >/dev/null || exit 1
CMD ["aggora-node", "node", "--bind", "0.0.0.0:8080", "--config", "config/default.toml", "--db-path", "/app/data/agc.sled", "--snapshot-path", "/app/snapshots"]
