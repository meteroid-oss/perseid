# build perseid
FROM docker.io/lukemathwalker/cargo-chef:latest-rust-1.88 AS chef
WORKDIR /app

FROM chef AS planner

COPY Cargo.toml .
COPY Cargo.lock .
COPY src /app/src
COPY build.rs /app/build.rs
COPY templates /app/templates
COPY runtime /app/runtime
COPY scripts /app/scripts

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS perseid-builder

# A static musl binary keeps native HTTP/DNS working on Alpine; gcompat cannot
# provide every resolver symbol used by a glibc-linked Rust HTTP client.
RUN rustup target add x86_64-unknown-linux-musl && \
    apt-get update && apt-get install -y --no-install-recommends musl-tools && \
    rm -rf /var/lib/apt/lists/*

COPY --from=planner /app/recipe.json recipe.json

RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json

COPY Cargo.toml .
COPY Cargo.lock .
COPY src /app/src
COPY build.rs /app/build.rs
COPY templates /app/templates
COPY runtime /app/runtime
COPY scripts /app/scripts

RUN cargo build --release --target x86_64-unknown-linux-musl --bin perseid


# Standalone formatters; no Go or Python runtime is required in the final image.
FROM golang:1.24.7-alpine3.21 AS go-formatter
FROM ghcr.io/astral-sh/ruff:0.14.10 AS python-formatter

# main image
FROM alpine:3.21
ENV CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup
ENV PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/opt/cargo/bin"
RUN apk add --no-cache openjdk21-jre-headless curl gcompat libgcc libstdc++ bash git github-cli

# Java formatter
RUN echo "25157797a0a972c2290b5bc71530c4f7ad646458025e3484412a6e5a9b8c9aa6 google-java-format-1.25.2-all-deps.jar" > google-java-format-1.25.2-all-deps.jar.sha256 && \
    curl -fsSL --output google-java-format-1.25.2-all-deps.jar "https://github.com/google/google-java-format/releases/download/v1.25.2/google-java-format-1.25.2-all-deps.jar" && \
    sha256sum google-java-format-1.25.2-all-deps.jar.sha256 -c && \
    rm google-java-format-1.25.2-all-deps.jar.sha256 && \
    mv google-java-format-1.25.2-all-deps.jar /usr/bin/  && \
    echo "#!/bin/sh" >> /usr/bin/google-java-format && \
    echo '/usr/bin/java -jar /usr/bin/google-java-format-1.25.2-all-deps.jar "$@"' >> /usr/bin/google-java-format && \
    chmod +x /usr/bin/google-java-format


# TypeScript/JavaScript formatter (biome)
ARG BIOME_DL_LINK="https://github.com/biomejs/biome/releases/download/%40biomejs%2Fbiome%402.1.4/biome-linux-x64-musl"
ARG BIOME_SHA256="6d6bd2213cffab0d68d741c0be466bcd21cd6f5eca1e0e5aac2a991bf9f17cf2"
RUN echo "${BIOME_SHA256} biome" > biome.sha256 && \
    curl -fsSL --output biome "${BIOME_DL_LINK}" && \
    sha256sum biome.sha256 -c && \
    rm biome.sha256 && \
    mv biome /usr/bin/  && \
    chmod +x /usr/bin/biome

# Rust formatter (rustfmt)
# Minimal install to reduce image size
RUN apk add --no-cache binutils && \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y \
    --profile minimal \
    --no-modify-path \
    --no-update-default-toolchain \
    --default-toolchain nightly-2025-02-27 \
    --component rustfmt && \
    rm -rf /opt/rustup/toolchains/nightly-*/lib/rustlib && \
    rm /opt/rustup/toolchains/nightly-*/bin/cargo* && \
    rm /opt/rustup/toolchains/nightly-*/bin/rust-* && \
    rm /opt/rustup/toolchains/nightly-*/bin/rustc && \
    rm /opt/rustup/toolchains/nightly-*/bin/rustdoc && \
    rm -rf /opt/rustup/toolchains/nightly-*/share && \
    strip /opt/rustup/toolchains/nightly-*/lib/librustc_driver-*.so && \
    apk del binutils

COPY --from=go-formatter /usr/local/go/bin/gofmt /usr/bin/gofmt
COPY --from=python-formatter /ruff /usr/bin/ruff

# perseid
COPY --from=perseid-builder /app/target/x86_64-unknown-linux-musl/release/perseid /usr/bin/

# Templates/runtimes/helpers are embedded in the native binary.
ENTRYPOINT ["perseid"]
ENV PERSEID_JAVA_FORMAT_JAR=/usr/bin/google-java-format-1.25.2-all-deps.jar
