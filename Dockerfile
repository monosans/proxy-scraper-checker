# syntax=docker.io/docker/dockerfile:1

# The builder runs natively on the build machine and cross-compiles for the
# target platform, so images for other architectures do not have to be built
# under QEMU emulation.
FROM --platform=$BUILDPLATFORM docker.io/rust:1-slim-trixie AS builder

WORKDIR /app

RUN rm -f /etc/apt/apt.conf.d/docker-clean \
  && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

ARG BUILDARCH TARGETARCH TARGETVARIANT

# Install a Debian cross toolchain for the target (the native one is already
# there) and write the environment that points cargo and cc at it.
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt,sharing=locked \
  case "${TARGETARCH}${TARGETVARIANT}" in \
    386) rust=i686-unknown-linux-gnu gnu=i686-linux-gnu deb=i386 ;; \
    amd64) rust=x86_64-unknown-linux-gnu gnu=x86_64-linux-gnu deb=amd64 ;; \
    armv7) rust=armv7-unknown-linux-gnueabihf gnu=arm-linux-gnueabihf deb=armhf ;; \
    arm64) rust=aarch64-unknown-linux-gnu gnu=aarch64-linux-gnu deb=arm64 ;; \
    ppc64le) rust=powerpc64le-unknown-linux-gnu gnu=powerpc64le-linux-gnu deb=ppc64el ;; \
    riscv64) rust=riscv64gc-unknown-linux-gnu gnu=riscv64-linux-gnu deb=riscv64 ;; \
    s390x) rust=s390x-unknown-linux-gnu gnu=s390x-linux-gnu deb=s390x ;; \
    *) echo "Unsupported platform: ${TARGETARCH}${TARGETVARIANT}" >&2 && exit 1 ;; \
  esac \
  && packages="cmake make" \
  && if [ "${TARGETARCH}" != "${BUILDARCH}" ]; then \
    packages="${packages} gcc-$(echo "${gnu}" | tr _ -) libc6-dev-${deb}-cross"; \
  fi \
  && apt-get update \
  && apt-get install -y --no-install-recommends ${packages} \
  && rustup target add "${rust}" \
  && env_rust="$(echo "${rust}" | tr a-z- A-Z_)" \
  && { \
    echo "export CARGO_BUILD_TARGET=${rust}"; \
    echo "export CARGO_TARGET_${env_rust}_LINKER=${gnu}-gcc"; \
    echo "export CC_$(echo "${rust}" | tr - _)=${gnu}-gcc"; \
    echo "export AR_$(echo "${rust}" | tr - _)=${gnu}-ar"; \
  } > /etc/cross-env

ARG TARGETPLATFORM

RUN --mount=source=src,target=src \
  --mount=source=Cargo.toml,target=Cargo.toml \
  --mount=source=Cargo.lock,target=Cargo.lock \
  --mount=type=cache,id=proxy-scraper-checker-target-${TARGETPLATFORM},target=/app/target,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
  . /etc/cross-env \
  && cargo build --release --locked \
  && cp "target/${CARGO_BUILD_TARGET}/release/proxy-scraper-checker" .


FROM docker.io/debian:trixie-slim AS final

WORKDIR /app

RUN rm -f /etc/apt/apt.conf.d/docker-clean \
  && echo 'Binary::apt::APT::Keep-Downloaded-Packages "true";' > /etc/apt/apt.conf.d/keep-cache

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt,sharing=locked \
  apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates

ARG \
  UID=1000 \
  GID=1000

RUN (getent group "${GID}" || groupadd --gid "${GID}" app) \
  && useradd --gid "${GID}" --no-log-init --create-home --uid "${UID}" app \
  && mkdir -p /home/app/.cache/proxy_scraper_checker \
  && chown "${UID}:${GID}" /home/app/.cache/proxy_scraper_checker

COPY --link LICENSE* ./

COPY --from=builder --link /app/proxy-scraper-checker .

USER app

CMD ["/app/proxy-scraper-checker"]
