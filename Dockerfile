FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS rust
FROM node:26.12.1-bookworm-slim@sha256:88e480776c6149ff1d098cd304f44bf18057fb5d284a3329b78162d29821e28c AS node
FROM ubuntu:24.04@sha256:2198e4c65076e9cb0b3d695219df667c0e3b0072da64ca3c2648aca17dcecaf5 AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential binutils ca-certificates cmake curl git pkg-config python3 systemd xz-utils \
    && rm -rf /var/lib/apt/lists/*
COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=rust /usr/local/cargo/bin /usr/local/cargo/bin
COPY --from=node /usr/local/ /usr/local/
ENV HOME=/home/ubuntu RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/home/ubuntu/.cargo \
    RUSTUP_TOOLCHAIN=1.99.1 RUSTUP_AUTO_INSTALL=0 PATH=/usr/local/cargo/bin:$PATH
RUN npm install --global pnpm@12.15.0 \
    && install -d -o 1000 -g 1000 /src /payload /verification /home/ubuntu/.cargo \
    && install -d -m 0700 -o 1000 -g 1000 /checks
WORKDIR /src
COPY --chown=1000:1000 . .
USER 1000:1000
ARG DOMUS_SOURCE_REVISION
RUN --mount=type=cache,id=domus-obscura-cargo-1.99.1-amd64,target=/home/ubuntu/.cargo,uid=1000,gid=1000,sharing=locked \
    --mount=type=cache,id=domus-obscura-target-1.99.1-amd64,target=/src/target,uid=1000,gid=1000,sharing=locked \
    python3 packaging/docker/test_health.py \
    && cargo task bootstrap && cargo task package-linux-container "$DOMUS_SOURCE_REVISION" \
    && cp -a target/linux-package/. /payload/ \
    && cp target/domus-obscura-0.1.0-linux-x86_64.tar.gz /verification/package.tar.gz

FROM builder AS package-verification
ENV TMPDIR=/checks
CMD ["python3", "packaging/linux/package-check.py", "/verification/package.tar.gz"]

FROM ubuntu:24.04@sha256:2198e4c65076e9cb0b3d695219df667c0e3b0072da64ca3c2648aca17dcecaf5 AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -m 0700 -o 1000 -g 1000 /storage /media
COPY --from=builder /payload /opt/domus-obscura
COPY --chmod=755 packaging/docker/health.sh /usr/local/bin/domus-health
ARG DOMUS_SOURCE_REVISION
ARG DOMUS_SOURCE_DIRTY=true
LABEL org.opencontainers.image.source="https://github.com/PlagueHO/domus-obscura" \
    org.opencontainers.image.revision="$DOMUS_SOURCE_REVISION" \
    org.opencontainers.image.version="0.1.0" \
    org.opencontainers.image.description="Domus Obscura Rust API, worker, and connected browser" \
    io.domus.source.dirty="$DOMUS_SOURCE_DIRTY"
ENV HOME=/storage LANG=C.UTF-8 \
    DOMUS_HEALTH_URL=http://127.0.0.1:8080/
WORKDIR /opt/domus-obscura
USER 1000:1000
HEALTHCHECK --interval=5s --timeout=5s --start-period=30s --retries=6 CMD ["domus-health"]
STOPSIGNAL SIGTERM
ENTRYPOINT ["/opt/domus-obscura/domus"]
CMD ["serve", "--storage-root", "/storage/application"]
