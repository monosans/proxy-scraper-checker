# syntax=docker.io/docker/dockerfile:1

FROM docker.io/rust:slim-bookworm AS builder

WORKDIR /app

RUN --mount=source=src,target=src \
    --mount=source=Cargo.toml,target=Cargo.toml \
    --mount=source=Cargo.lock,target=Cargo.lock \
    --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked \
    && cp target/release/proxy-scraper-checker .


FROM docker.io/debian:bookworm-slim as runner

WORKDIR /app

RUN groupadd --gid 1000 app \
  && useradd --gid 1000 --no-log-init --create-home --uid 1000 app \
  && mkdir -p /home/app/.cache/proxy_scraper_checker \
  && chown 1000:1000 /home/app/.cache/proxy_scraper_checker

COPY --from=builder --chown=1000:1000 --link /app/proxy-scraper-checker .

USER app

CMD ["/app/proxy-scraper-checker"]
