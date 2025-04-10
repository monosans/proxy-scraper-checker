#!/usr/bin/env bash

set -euo pipefail

project_name="proxy-scraper-checker"
base_path="${HOME}"
install_path="${base_path}/${project_name}"
download_path="${TMPDIR}/${project_name}.zip"

abi=$(getprop ro.product.cpu.abi)

case "${abi}" in
  "arm64-v8a")
    target="aarch64-linux-android"
    ;;
  "armeabi-v7a")
    if grep -qi 'neon' /proc/cpuinfo; then
      target="thumbv7neon-linux-androideabi"
    else
      target="armv7-linux-androideabi"
    fi
    ;;
  "armeabi")
    target="arm-linux-androideabi"
    ;;
  "x86")
    target="i686-linux-android"
    ;;
  "x86_64")
    target="x86_64-linux-android"
    ;;
  *)
    echo "Unsupported CPU ABI: ${abi}" >&2
    exit 1
    ;;
esac

curl -fLo "${download_path}" "https://nightly.link/monosans/proxy-scraper-checker/workflows/ci/main/proxy-scraper-checker-${target}.zip"
mkdir "${install_path}"
unzip -d "${install_path}" "${download_path}"
rm -f "${download_path}"
printf "%s installed successfully.\nRun 'cd %s && ./%s'.\n" "${project_name}" "${install_path}" "${project_name}"
