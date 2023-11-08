#!/bin/sh
python3 -m pip3 install -U --no-cache-dir --disable-pip-version-check setuptools wheel &&
python3 -m pip3 install -U --no-cache-dir --disable-pip-version-check -r requirements-termux.txt &&
python3 -m proxy-here-Noobs
