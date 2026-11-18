#!/usr/bin/env bash
# Builds flake packages in the nix container, one after another (a failure does not stop the next); each
# result is a GC root in the volume (/nix/var/nix/gcroots/atlas-<name>). Skips a package when less than
# 9 GB is free in $HOME.   nix-build-atlas.sh SRC_DIR atlas-perf atlas
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
src=${1:?flake source dir}; shift
rc=0
for p in "$@"; do
  echo "=== $p $(date)"
  avail=$(df --output=avail -BG "$HOME" | tail -1 | tr -dc 0-9)
  if ((avail < 9)); then echo "=== $p skipped: only ${avail} GB free on the host"; rc=1; continue; fi
  r=0; nice "$here/nix-docker.sh" "$src" "nix build -L --print-out-paths --out-link /nix/var/nix/gcroots/atlas-$p path:/src#$p" || r=$?
  ((r)) && rc=1
  echo "=== $p done rc=$r $(date)"
done
exit $rc
