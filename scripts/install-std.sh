#!/usr/bin/env bash
# Install a source-tree copy of Ruddy's standard library without exposing a
# partially copied project. The destination is replaced only after staging has
# completed on the same filesystem.
set -euo pipefail

source=${1:?usage: install-std.sh SOURCE}
if [[ ! -f "$source/Ruddy.toml" || ! -f "$source/main.hc" ]]; then
  printf 'standard-library source %q is not a Ruddy project\n' "$source" >&2
  exit 1
fi

if [[ -n ${RUDDY_HOME:-} ]]; then
  home=$RUDDY_HOME
elif [[ -n ${HOME:-} ]]; then
  home=$HOME/.ruddy
else
  echo 'could not determine Ruddy home; set RUDDY_HOME' >&2
  exit 1
fi

mkdir -p "$home"
staging=$(mktemp -d "$home/.std.install.XXXXXX")
backup=
committed=false
cleanup() {
  status=$?
  if [[ $committed != true && -n $backup && -e $backup && ! -e $home/std ]]; then
    mv "$backup" "$home/std" || true
  fi
  rm -rf "$staging"
  if [[ $committed == true && -n $backup ]]; then
    rm -rf "$backup"
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# `source/.` includes dotfiles while keeping the staging directory itself on the
# destination filesystem, so the final rename cannot cross a mount boundary.
cp -R "$source/." "$staging/"
# Build products are local state, not part of the installed source project.
# Remove a directory or symlink with that name without following it.
rm -rf "$staging/build"

if [[ -e $home/std || -L $home/std ]]; then
  backup=$(mktemp -d "$home/.std.backup.XXXXXX")
  rmdir "$backup"
  mv "$home/std" "$backup"
fi

mv "$staging" "$home/std"
committed=true
printf 'installed Ruddy standard library in %s\n' "$home/std"
