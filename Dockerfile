# perseid with every pinned formatter, for self-hosted runners and non-GitHub CI.
FROM rust:1.98-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked
# rustfmt only needs its binary and two shared libraries, not a whole toolchain.
RUN rustup component add rustfmt \
    && toolchain="$(rustc --print sysroot)" \
    && mkdir -p /out/bin /out/lib \
    && cp "$toolchain/bin/rustfmt" /out/bin/ \
    && cp "$toolchain"/lib/librustc_driver-*.so "$toolchain"/lib/libLLVM*.so* /out/lib/

FROM golang:1.24-bookworm AS go

FROM debian:bookworm-slim
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git gh \
       $([ "$TARGETARCH" = amd64 ] || echo default-jre-headless) \
    && rm -rf /var/lib/apt/lists/*
RUN set -eux; \
    case "$TARGETARCH" in amd64) biome=x64; ruff=x86_64 ;; *) biome=arm64; ruff=aarch64 ;; esac; \
    curl -fsSL -o /usr/local/bin/biome "https://github.com/biomejs/biome/releases/download/%40biomejs%2Fbiome%402.1.4/biome-linux-$biome"; \
    curl -fsSL "https://github.com/astral-sh/ruff/releases/download/0.14.10/ruff-$ruff-unknown-linux-gnu.tar.gz" \
      | tar -xz --strip-components 1 -C /usr/local/bin "ruff-$ruff-unknown-linux-gnu/ruff"; \
    gjf=https://github.com/google/google-java-format/releases/download/v1.25.2; \
    if [ "$TARGETARCH" = amd64 ]; then \
      curl -fsSL -o /usr/local/bin/google-java-format "$gjf/google-java-format_linux-x86-64"; \
    else \
      curl -fsSL -o /opt/google-java-format.jar "$gjf/google-java-format-1.25.2-all-deps.jar"; \
      printf '#!/bin/sh\nexec java -jar /opt/google-java-format.jar "$@"\n' > /usr/local/bin/google-java-format; \
    fi; \
    chmod +x /usr/local/bin/biome /usr/local/bin/google-java-format
COPY --from=build /out/ /usr/local/
COPY --from=go /usr/local/go/bin/gofmt /usr/local/bin/gofmt
COPY --from=build /src/target/release/perseid /usr/local/bin/perseid
RUN git config --system --add safe.directory '*'
WORKDIR /work
ENTRYPOINT ["perseid"]
