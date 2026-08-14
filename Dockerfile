# ── Stage 1: Build ──────────────────────────────────────────────────────────
FROM rust:1.88-slim AS build

# Native deps required by the dependency graph:
#   - build-essential + cmake  → aws-lc-sys / audiopus_sys (Opus built from source)
#   - pkg-config + libasound2-dev → alsa-sys (rodio / cpal audio)
#   - perl → ring (C/asm build)
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential cmake pkg-config perl libasound2-dev \
    && rm -rf /var/lib/apt/lists/*

# CMake 4.x dropped compatibility with projects declaring cmake_minimum_required
# below 3.5 (the bundled Opus CMakeLists.txt does). Allow configuring anyway.
ENV CMAKE_POLICY_VERSION_MINIMUM=3.5

WORKDIR /app

# Copy the workspace manifests and sources.
COPY Cargo.toml Cargo.lock ./
COPY api ./api
COPY relay ./relay
COPY test ./test

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --bin noxbot

# ── Stage 2: Runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates libasound2 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=build /app/target/release/noxbot ./noxbot
COPY docker-entrypoint.sh ./docker-entrypoint.sh

ENTRYPOINT ["sh", "/app/docker-entrypoint.sh"]
