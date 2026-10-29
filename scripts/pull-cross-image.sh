#!/usr/bin/env bash
set -euo pipefail

# scripts/pull-cross-image.sh <target> — pre-pull the digest-pinned cross image
# for <target> from Cross.toml, retrying the pull only.
#
# WHY: main @ 53318ef went red when ghcr.io answered "not found" for the pinned
# aarch64 image digest (run 40811478316, 2026-10-29); the same digest resolved
# minutes before and after. cross pulls inside `cross build`, so one registry
# blip failed the build step. Pulling first lets a transient registry error be
# retried without ever retrying the compile. A target without a pinned
# `image =` (the x86_64 leg builds its own Dockerfile) is a no-op. Exhausting
# the attempts fails the step: a digest that is really gone stays red.
#
# Env: CROSS_TOML (default Cross.toml), PULL_ATTEMPTS (default 3),
# PULL_RETRY_DELAY_S (default 20; doubled after each failed attempt).

target=${1:?usage: pull-cross-image.sh <target>}
cross_toml=${CROSS_TOML:-Cross.toml}
attempts=${PULL_ATTEMPTS:-3}
delay=${PULL_RETRY_DELAY_S:-20}

image=$(awk -v section="[target.${target}]" '
  /^\[/ { in_section = ($0 == section); next }
  in_section && /^[[:space:]]*image[[:space:]]*=/ {
    sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; exit
  }
' "$cross_toml")

if [[ -z "$image" ]]; then
  echo "No pinned cross image for ${target} in ${cross_toml}; nothing to pull."
  exit 0
fi

for ((i = 1; i <= attempts; i++)); do
  if docker pull "$image"; then
    exit 0
  fi
  if ((i < attempts)); then
    echo "::warning::docker pull ${image} failed (attempt ${i}/${attempts}); retrying in ${delay}s"
    sleep "$delay"
    delay=$((delay * 2))
  fi
done

echo "::error::docker pull ${image} failed after ${attempts} attempts"
exit 1
