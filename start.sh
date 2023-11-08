#!/bin/sh
python -m pip3 install -U --no-cache-dir --disable-pip-version-check pip setuptools wheel &&
python -m pip3 install -U --no-cache-dir --disable-pip-version-check -r requirements.txt &&
python -m proxy_scraper_checker
