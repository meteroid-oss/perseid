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

FROM mcr.microsoft.com/dotnet/sdk:8.0-bookworm-slim AS csharpier
RUN dotnet tool install csharpier --version 1.3.0 --tool-path /opt/csharpier

FROM debian:bookworm-slim
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git gh libstdc++6 \
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
# csharpier is a .NET tool: the runtime alone is enough to run it.
COPY --from=mcr.microsoft.com/dotnet/runtime:8.0-bookworm-slim /usr/share/dotnet /usr/share/dotnet
COPY --from=csharpier /opt/csharpier /opt/csharpier
ENV DOTNET_ROOT=/usr/share/dotnet DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
RUN ln -s /opt/csharpier/csharpier /usr/local/bin/csharpier
COPY --from=build /src/target/release/perseid /usr/local/bin/perseid
RUN git config --system --add safe.directory '*'
WORKDIR /work
ENTRYPOINT ["perseid"]
