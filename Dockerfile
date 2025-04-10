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

ARG \
  UID=1000 \
  GID=1000

RUN groupadd --gid ${GID} app \
  && useradd --gid ${GID} --no-log-init --create-home --uid ${UID} app \
  && mkdir -p /home/app/.cache/proxy_scraper_checker \
  && chown ${UID}:${GID} /home/app/.cache/proxy_scraper_checker

COPY --from=builder --chown=${UID}:${GID} --link /app/proxy-scraper-checker .

USER app

CMD ["/app/proxy-scraper-checker"]
