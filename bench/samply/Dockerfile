# syntax=docker.io/docker/dockerfile:1

FROM docker.io/rust:1-slim-trixie AS builder

WORKDIR /app

RUN rm -f /etc/apt/apt.conf.d/docker-clean \
  && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt,sharing=locked \
  apt-get update \
  && apt-get install -y --no-install-recommends cmake make

ARG TARGETPLATFORM

# A RUN that clones samply would stay cached on an old commit. ADD resolves
# the branch on every build, so the layer is rebuilt when main moves.
ADD --link https://github.com/mstange/samply.git#main /samply

RUN --mount=type=cache,id=samply-target-${TARGETPLATFORM},target=/samply/target,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  cargo install --locked --path /samply/samply

RUN --mount=source=src,target=src \
  --mount=source=Cargo.toml,target=Cargo.toml \
  --mount=source=Cargo.lock,target=Cargo.lock \
  --mount=type=cache,id=proxy-scraper-checker-profiling-target-${TARGETPLATFORM},target=/app/target,sharing=locked \
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
