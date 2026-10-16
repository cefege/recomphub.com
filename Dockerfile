FROM rust:1-bookworm AS builder
WORKDIR /app

RUN curl -fsSL https://github.com/cargo-bins/cargo-binstall/releases/latest/download/cargo-binstall-x86_64-unknown-linux-musl.tgz \
 | tar -xz -C /usr/local/cargo/bin \
 && cargo binstall dioxus-cli@0.7.10 -y --strategies compile \
 && rustup target add wasm32-unknown-unknown

# Pinned standalone yt-dlp, verified against the release checksum for its architecture.
ARG YTDLP_VERSION=2026.08.19
ARG TARGETARCH
RUN case "${TARGETARCH:-amd64}" in \
      amd64) asset=yt-dlp_linux; sha=58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a ;; \
      arm64) asset=yt-dlp_linux_aarch64; sha=b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc ;; \
      *) echo "unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac \
 && curl -fsSL -o /usr/local/bin/yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}/${asset}" \
 && echo "${sha}  /usr/local/bin/yt-dlp" | sha256sum -c - \
 && chmod 0755 /usr/local/bin/yt-dlp

COPY . .
RUN dx bundle --web --release --package recomphub
RUN cargo build --release -p recomphub-worker

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/dx/recomphub/release/web/ /usr/local/app/
COPY --from=builder /app/target/release/worker /usr/local/app/worker
COPY --from=builder /usr/local/bin/yt-dlp /usr/local/bin/yt-dlp
WORKDIR /usr/local/app
ENV IP=0.0.0.0 PORT=8080 MEDIA_DIR=/data/media
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/app/server"]
LABEL org.opencontainers.image.source=https://github.com/cefege/recomphub.com
