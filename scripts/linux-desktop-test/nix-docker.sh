#!/usr/bin/env bash
# Run a command with Nix (flakes on) in the nixos/nix container; the store lives in the docker volume
# atlas-nix-store, so builds are kept between runs. The first argument is mounted at /src (read-write).
#   nix-docker.sh DIR 'nix build -L path:/src#atlas'
set -euo pipefail
src=$(realpath "${1:?dir}"); shift
exec docker run --rm --name "atlas-nix-$$" --device /dev/kvm -v atlas-nix-store:/nix -v "$src":/src -w /src \
  -e NIX_CONFIG=$'experimental-features = nix-command flakes\nsystem-features = kvm nixos-test benchmark big-parallel\nmax-jobs = 1\ncores = 4\nwarn-dirty = false' \
  nixos/nix:latest sh -c "$*"
