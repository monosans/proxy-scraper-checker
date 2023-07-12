#!/bin/sh
path=~/proxy-scraper-checker
pkg upgrade --yes -o Dpkg::Options::='--force-confdef' &&
pkg install --yes python python-pip &&
if [ -d "${path}" ]; then
    rm -rfi "${path}"
fi &&
curl -fsSL 'https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip' | unzip - -d "${path}" &&
python3 -m pip install -U --no-cache-dir --disable-pip-version-check setuptools wheel &&
python3 -m pip install -U --no-cache-dir --disable-pip-version-check -r "${path}"/requirements.txt &&
printf "proxy-scraper-checker installed successfully.\nRun 'cd %s && sh start-termux.sh'." "${path}"
