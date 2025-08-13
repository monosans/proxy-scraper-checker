# syntax=docker.io/docker/dockerfile:1

FROM docker.io/rust:slim-trixie AS builder

WORKDIR /app

RUN --mount=source=src,target=src \
  --mount=source=Cargo.toml,target=Cargo.toml \
  --mount=source=Cargo.lock,target=Cargo.lock \
  --mount=type=cache,target=/app/target,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  cargo build --features mimalloc --release --locked \
  && cp target/release/proxy-scraper-checker .


FROM docker.io/debian:trixie-slim AS final

WORKDIR /app

ARG \
  UID=1000 \
  GID=1000

RUN (getent group "${GID}" || groupadd --gid "${GID}" app) \
  && useradd --gid "${GID}" --no-log-init --create-home --uid "${UID}" app \
  && mkdir -p /home/app/.cache/proxy_scraper_checker \
  && chown ${UID}:${GID} /home/app/.cache/proxy_scraper_checker

COPY --from=builder --chown=${UID}:${GID} --link /app/proxy-scraper-checker .

USER app

CMD ["/app/proxy-scraper-checker"]
