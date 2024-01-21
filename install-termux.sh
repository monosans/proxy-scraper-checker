#!/bin/sh
base_path=~
path="${base_path}/proxy-scraper-checker"
download_path="${PREFIX}/tmp/proxy-scraper-checker.zip"

pkg upgrade --yes -o Dpkg::Options::='--force-confdef' &&
pkg install --yes python python-pip &&
if [ -d "${path}" ]; then
    rm -rf --interactive=once "${path}"
fi &&
curl -fsSLo "${download_path}" 'https://github.com/monosans/proxy-scraper-checker/archive/refs/heads/main.zip' &&
unzip -d "${base_path}" "${download_path}" &&
mv "${path}-main" "${path}" &&
printf "proxy-scraper-checker installed successfully.\nRun 'cd %s && sh start-termux.sh'.\n" "${path}"
