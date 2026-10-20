FROM rust:1-bookworm AS builder
WORKDIR /app
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential git meson nasm ninja-build pkg-config \
 && rm -rf /var/lib/apt/lists/*
ENV SYSTEM_DEPS_DAV1D_BUILD_INTERNAL=always


RUN curl -fsSL https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-musl.tgz \
 | tar -xz -C /usr/local/cargo/bin \
 && cargo binstall dioxus-cli@0.7.10 -y --locked --strategies compile \
 && rustup target add wasm32-unknown-unknown


COPY . .
RUN dx bundle --web --release --package recomphub
RUN cargo build --release -p recomphub-worker

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/dx/recomphub/release/web/ /usr/local/app/
COPY --from=builder /app/target/release/worker /usr/local/app/worker
WORKDIR /usr/local/app
ENV IP=0.0.0.0 PORT=8080 MEDIA_DIR=/data/media
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/app/server"]
LABEL org.opencontainers.image.source=https://github.com/cefege/recomphub.com
