FROM docker.io/python:3.12-slim-bookworm as python-base-stage

ENV \
  PIP_DISABLE_PIP_VERSION_CHECK=1 \
  PIP_NO_CACHE_DIR=1 \
  PIP_NO_COLOR=1 \
  PIP_NO_INPUT=1 \
  PIP_PROGRESS_BAR=off \
  PIP_ROOT_USER_ACTION=ignore \
  PIP_UPGRADE=1 \
  PYTHONDONTWRITEBYTECODE=1 \
  PYTHONUNBUFFERED=1

WORKDIR /app


FROM python-base-stage as python-build-stage

ENV \
  POETRY_NO_ANSI=1 \
  POETRY_NO_CACHE=1 \
  POETRY_NO_INTERACTION=1

RUN apt-get update \
  && apt-get install -y --no-install-recommends build-essential \
  && apt-get purge -y --auto-remove -o APT::AutoRemove::RecommendsImportant=false \
  && rm -rf /var/lib/apt/lists/* \
  && pip install poetry poetry-plugin-export

COPY ./poetry.lock ./pyproject.toml ./

RUN poetry export --without-hashes --only=main --extras=non-termux | \
  pip wheel --wheel-dir /usr/src/app/wheels -r /dev/stdin


FROM python-base-stage as python-run-stage

RUN apt-get update \
  && apt-get install -y --no-install-recommends -y tini \
  && apt-get purge -y --auto-remove -o APT::AutoRemove::RecommendsImportant=false \
  && rm -rf /var/lib/apt/lists/*

COPY --from=python-build-stage /usr/src/app/wheels /wheels/

RUN pip install --no-index --find-links /wheels/ /wheels/* \
  && rm -rf /wheels/

ARG GID UID

RUN groupadd --gid "${GID}" --system app \
  && useradd --gid app --no-log-init --create-home --system --uid "${UID}" app \
  && mkdir -p /home/app/.cache/proxy_scraper_checker \
  && chown app:app /home/app/.cache/proxy_scraper_checker

ENV IS_DOCKER=1

COPY . .

USER app

ENTRYPOINT ["tini", "--"]

CMD ["python", "-m", "proxy_scraper_checker"]
