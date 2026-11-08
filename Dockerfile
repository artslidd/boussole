# syntax=docker/dockerfile:1
#
# Boussole server image: only `-p boussole-server` is compiled; the Slint client is never
# built. Build from the repository root:
#
#   docker build -t boussole-server .
#   podman build --format docker -t boussole-server .   # --format docker keeps HEALTHCHECK
#
# ---- build ---------------------------------------------------------------------------------
FROM rust:1.99-slim-bookworm AS build
WORKDIR /src

# Strip symbols in this build only (keeps the image small without touching the workspace's
# own release profile, which the desktop client also uses).
ENV CARGO_PROFILE_RELEASE_STRIP=symbols

# The workspace root is also the Slint client package. Cargo needs every member's manifest and
# a target file to load the workspace, but `-p boussole-server` never compiles the client (nor
# runs its build.rs, which is not even copied): an empty src/lib.rs is all it takes.
COPY Cargo.toml Cargo.lock ./
COPY crates/boussole-core/Cargo.toml crates/boussole-core/Cargo.toml
COPY crates/boussole-protocol/Cargo.toml crates/boussole-protocol/Cargo.toml
COPY crates/boussole-server/Cargo.toml crates/boussole-server/Cargo.toml

# Layer 1: dependencies only, built from stub sources so it is cached until a manifest or
# Cargo.lock changes.
RUN mkdir -p src crates/boussole-core/src crates/boussole-protocol/src crates/boussole-server/src \
 && touch src/lib.rs crates/boussole-core/src/lib.rs crates/boussole-protocol/src/lib.rs crates/boussole-server/src/lib.rs \
 && echo 'fn main() {}' > crates/boussole-server/src/main.rs \
 && cargo build --release --locked -p boussole-server

# Layer 2: the real sources. `touch` makes cargo rebuild the stubbed crates even though the
# copied files may carry older timestamps than the stub artefacts.
COPY crates crates
RUN find crates -name '*.rs' -path '*/src/*' -exec touch {} + \
 && cargo build --release --locked -p boussole-server

# ---- runtime -------------------------------------------------------------------------------
# distroless: glibc + libgcc only, no shell, no package manager, runs as uid 65532.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /src/target/release/boussole-server /usr/local/bin/boussole-server

ENV BOUSSOLE_BIND=0.0.0.0:7878
EXPOSE 7878
USER nonroot:nonroot

# No curl in distroless: the server binary probes its own /healthz.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD ["/usr/local/bin/boussole-server", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/boussole-server"]
