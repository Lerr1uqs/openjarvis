#!/usr/bin/env bash
set -euo pipefail

# This helper is executed through one symlink created inside each sandbox fixture root.
# Use the symlink directory as the host workspace root so the test can avoid rewriting
# and immediately executing a brand-new shell script.
script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
helper_bin="${script_dir}/openjarvis-test-bin"

if [[ ! -x "${helper_bin}" ]]; then
  echo "missing openjarvis test helper: ${helper_bin}" >&2
  exit 1
fi

while [[ $# -gt 0 ]]; do
  if [[ "$1" == "--" ]]; then
    shift
    break
  fi
  shift
done

if [[ $# -eq 0 ]]; then
  echo "missing -- separator" >&2
  exit 1
fi

shift
args=()
while [[ $# -gt 0 ]]; do
  if [[ "$1" == "--workspace-root" ]]; then
    shift
    args+=("--workspace-root" "${script_dir}")
    if [[ $# -gt 0 ]]; then
      shift
    fi
    continue
  fi
  args+=("$1")
  shift
done

exec "${helper_bin}" "${args[@]}"
