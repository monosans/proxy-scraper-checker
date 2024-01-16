#!/bin/sh
python3 -m venv --upgrade-deps .venv &&
.venv/bin/python3 -m pip install -U --disable-pip-version-check --editable .[full] &&
.venv/bin/python3 -m proxy_scraper_checker
