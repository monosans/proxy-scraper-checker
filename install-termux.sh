#!/bin/sh
base_path=~
path="${base_path}/proxy-here-Noobs"
download_path="${PREFIX}/tmp/proxy-here-Noobs.zip"

pkg upgrade --yes -o Dpkg::Options::='--force-confdef' &&
pkg install --yes python python-pip &&
if [ -d "${path}" ]; then
    rm -rf --interactive=once "${path}"
fi &&
curl -fsSLo "${download_path}" 'https://github.com/victorgeel/proxy-here-Noobs/archive/refs/heads/modified.zip' &&
unzip -d "${base_path}" "${download_path}" &&
mv "${path}-modified" "${path}" &&
python3 -m pip install -U --no-cache-dir --disable-pip-version-check setuptools wheel &&
python3 -m pip install -U --no-cache-dir --disable-pip-version-check -r "${path}/requirements-termux.txt" &&
printf "proxy-here-Noobs installed successfully.\nRun 'cd %s && sh start-termux.sh'.\n" "${path}"
