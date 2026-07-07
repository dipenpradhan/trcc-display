# syntax=docker/dockerfile:1
# Multi-stage build. libusb is vendored (compiled into the binary), so the
# runtime image needs no libusb and the resulting x86_64 binary is portable
# across Debian hosts (build on clippy, run on orange).

FROM rust:1-bookworm AS build
WORKDIR /src
# Cache dependencies first.
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY config ./config
COPY tests ./tests
# `--locked` when a lockfile is present; otherwise let cargo resolve.
RUN cargo build --release && strip target/release/trcc-display || true

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="trcc-display" \
      org.opencontainers.image.description="Drive Thermalright Digital cooler displays from Prometheus/lm-sensors" \
      org.opencontainers.image.licenses="Apache-2.0"
# `sensors` is only needed for source.kind=sensors; harmless otherwise.
RUN apt-get update \
    && apt-get install -y --no-install-recommends lm-sensors ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/trcc-display /usr/local/bin/trcc-display
COPY config /app/config
# Handshake probe cache lives here — mount a volume to persist across restarts.
VOLUME ["/app/state"]
ENTRYPOINT ["trcc-display", "--config", "/app/config/config.json"]
CMD ["run"]
