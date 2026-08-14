#!/usr/bin/env bash
set -euo pipefail

# Outputs build metadata consumed by the CI workflow:
#   version  - git tag version (v1.2.3 -> 1.2.3), otherwise "dev"
#   buildid  - GitHub Actions run number
#   platform - Rust host target triple (e.g. x86_64-unknown-linux-gnu)
#   ext      - ".exe" on Windows, empty elsewhere

if [[ "$GITHUB_REF" == refs/tags/v* ]]; then
  echo "version=${GITHUB_REF#refs/tags/v}" >> "$GITHUB_OUTPUT"
else
  echo "version=dev" >> "$GITHUB_OUTPUT"
fi

echo "buildid=${GITHUB_RUN_NUMBER}" >> "$GITHUB_OUTPUT"
echo "platform=$(rustc --print host-tuple)" >> "$GITHUB_OUTPUT"

if [[ "$RUNNER_OS" == "Windows" ]]; then
  echo "ext=.exe" >> "$GITHUB_OUTPUT"
else
  echo "ext=" >> "$GITHUB_OUTPUT"
fi
