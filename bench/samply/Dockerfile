# syntax=docker.io/docker/dockerfile:1

FROM docker.io/rust:1-slim-trixie AS builder

WORKDIR /app

RUN rm -f /etc/apt/apt.conf.d/docker-clean \
  && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt,sharing=locked \
  apt-get update \
  && apt-get install -y --no-install-recommends cmake make

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  cargo install --locked --git https://github.com/mstange/samply samply

RUN --mount=source=src,target=src \
  --mount=source=Cargo.toml,target=Cargo.toml \
  --mount=source=Cargo.lock,target=Cargo.lock \
  --mount=type=cache,target=/app/target,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  cargo build --profile profiling --locked \
  && cp target/profiling/proxy-scraper-checker .


FROM docker.io/debian:trixie-slim AS final

WORKDIR /app

RUN rm -f /etc/apt/apt.conf.d/docker-clean \
  && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt,sharing=locked \
  apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates

COPY --from=builder --link /usr/local/cargo/bin/samply .
COPY --from=builder --link /app/proxy-scraper-checker .

CMD ["/app/samply", "record", "--rate", "1750", "--presymbolicate", "--save-only", "-o", "/app/out/samply-record.json", "/app/proxy-scraper-checker"]
