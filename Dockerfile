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
COPY --from=build /src/target/release/perseid /usr/local/bin/perseid
RUN perseid tools install csharp --dir /opt/csharpier && rm /opt/csharpier/oasdiff

FROM debian:bookworm-slim
ARG TARGETARCH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git gh libstdc++6 \
       $([ "$TARGETARCH" = amd64 ] || echo default-jre-headless) \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/ /usr/local/
COPY --from=go /usr/local/go/bin/gofmt /usr/local/bin/gofmt
# csharpier is a .NET tool: the runtime alone is enough to run it.
COPY --from=mcr.microsoft.com/dotnet/runtime:8.0-bookworm-slim /usr/share/dotnet /usr/share/dotnet
COPY --from=csharpier /opt/csharpier /opt/csharpier
ENV DOTNET_ROOT=/usr/share/dotnet DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1
RUN ln -s /opt/csharpier/csharpier /usr/local/bin/csharpier
COPY --from=build /src/target/release/perseid /usr/local/bin/perseid
RUN perseid tools install typescript python java --dir /usr/local/bin
RUN git config --system --add safe.directory '*'
WORKDIR /work
ENTRYPOINT ["perseid"]
