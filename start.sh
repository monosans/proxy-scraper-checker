#!/bin/sh
python3 -m pip install -U --no-cache-dir --disable-pip-version-check pip setuptools wheel && python3 -m pip install -U --no-cache-dir --disable-pip-version-check -r requirements.txt && python3 -m proxy_scraper_checker
