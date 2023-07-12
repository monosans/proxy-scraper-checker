#!/bin/sh
pkg upgrade --yes -o Dpkg::Options::='--force-confdef' &&
pkg install --yes python python-pip &&
rm -rfi ~/proxy-scraper-checker &&
curl 'https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip' | unzip - -d ~/proxy-scraper-checker &&
python3 -m pip install -U --no-cache-dir --disable-pip-version-check setuptools wheel &&
python3 -m pip install -U --no-cache-dir --disable-pip-version-check -r ~/proxy-scraper-checker/requirements.txt &&
echo "proxy-scraper-checker installed successfully.\nRun 'cd ~/proxy-scraper-checker && sh start-termux.sh'."
