FROM rust:1.85-bookworm AS builder

WORKDIR /workspace
COPY . .
RUN cargo build --release --package scg-node

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 scg \
    && install --directory --owner scg --group scg /var/lib/scg
COPY --from=builder /workspace/target/release/scg-node /usr/local/bin/scg-node

USER scg
WORKDIR /home/scg
ENV SCG_BIND=0.0.0.0:8080
ENV SCG_DATA_DIR=/var/lib/scg

VOLUME ["/var/lib/scg"]
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8080/healthz || exit 1

ENTRYPOINT ["scg-node"]
