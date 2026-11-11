#!/usr/bin/env bash
set -euo pipefail

# Probe official Ubuntu mirrors before apt retries an unreachable archive for minutes.
# Packages still use the image's Ubuntu signing key and normal apt verification.
source /etc/os-release
for mirror in https://archive.ubuntu.com/ubuntu https://mirrors.kernel.org/ubuntu; do
    if curl --fail --silent --show-error --connect-timeout 5 --max-time 15 \
        "$mirror/dists/$VERSION_CODENAME/InRelease" --output "$RUNNER_TEMP/ubuntu-inrelease"; then
        sudo sed -i "s|mirror+file:/etc/apt/apt-mirrors.txt|$mirror|g" /etc/apt/sources.list.d/ubuntu.sources
        sudo apt-get update -qq -o APT::Update::Error-Mode=any -o Acquire::Retries=2 -o Acquire::https::Timeout=30
        exit 0
    fi
done

echo 'No Ubuntu archive is reachable.' >&2
exit 1
