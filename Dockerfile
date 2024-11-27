# syntax=docker/dockerfile:1

FROM docker.io/python:3.12-slim-bookworm AS python-base-stage

ENV \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONUNBUFFERED=1

WORKDIR /app


FROM python-base-stage AS python-build-stage

RUN apt-get update \
  && apt-get install -y --no-install-recommends build-essential \
  && apt-get purge -y --auto-remove -o APT::AutoRemove::RecommendsImportant=false \
  && rm -rf /var/lib/apt/lists/*

ENV \
  UV_COMPILE_BYTECODE=1 \
  UV_LINK_MODE=copy

RUN --mount=from=ghcr.io/astral-sh/uv,source=/uv,target=/bin/uv \
  --mount=type=cache,target=/root/.cache/uv,sharing=locked \
  --mount=source=pyproject.toml,target=pyproject.toml \
  --mount=source=uv.lock,target=uv.lock \
  uv sync --no-dev --no-install-project --frozen


FROM python-base-stage AS python-run-stage

RUN apt-get update \
  && apt-get install -y --no-install-recommends tini \
  && apt-get purge -y --auto-remove -o APT::AutoRemove::RecommendsImportant=false \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd --gid 1000 app \
  && useradd --gid 1000 --no-log-init --create-home --uid 1000 app \
  && mkdir -p /home/app/.cache/proxy_scraper_checker \
  && chown 1000:1000 /home/app/.cache/proxy_scraper_checker

COPY --from=python-build-stage --chown=1000:1000 --link /app/.venv /app/.venv

ENV PATH="/app/.venv/bin:$PATH"

ENV IS_DOCKER=1

USER app

COPY --chown=1000:1000 . .

ENTRYPOINT ["tini", "--"]

CMD ["python", "-m", "proxy_scraper_checker"]
