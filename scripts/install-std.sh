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

replacement=false
if [[ -e $home/std || -L $home/std ]]; then
  replacement=true
  mv_help=$(mv --help 2>&1 || true)
  if [[ $(uname -s) != Linux || $mv_help != *--exchange* ]]; then
    echo 'cannot atomically replace the Ruddy standard library: replacement requires Linux and GNU mv with --exchange support' >&2
    echo 'remove the existing std directory to perform a portable first install' >&2
    exit 1
  fi

  # Diagnose kernel/filesystem support before copying the source tree. The
  # probe is beside the destination, so it exercises the same filesystem and
  # the same renameat2(RENAME_EXCHANGE) operation as the eventual commit.
  probe=$(mktemp -d "$home/.std.exchange.XXXXXX")
  mkdir "$probe/left" "$probe/right"
  if ! mv --exchange --no-copy -T -- "$probe/left" "$probe/right"; then
    rm -rf -- "$probe"
    echo 'cannot atomically replace the Ruddy standard library: this Linux filesystem does not support directory exchange' >&2
    echo 'remove the existing std directory to perform a portable first install' >&2
    exit 1
  fi
  rm -rf -- "$probe"
fi

staging=$(mktemp -d "$home/.std.install.XXXXXX")
cleanup() {
  status=$?
  # Before the commit this removes the uninstalled tree. After an exchange,
  # this same path names the previous installation instead.
  rm -rf -- "$staging"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# Copy only the manifest and Ruddy source tree. In particular, build products,
# repository metadata, and editor files never become part of the installation.
cp "$source/Ruddy.toml" "$staging/Ruddy.toml"
while IFS= read -r -d '' file; do
  relative=${file#"$source"/}
  mkdir -p "$staging/$(dirname "$relative")"
  cp "$file" "$staging/$relative"
done < <(find "$source" -path "$source/build" -prune -o -type f -name '*.hc' -print0)

# Tests use this barrier to interrupt a fully staged install before its commit.
# It is intentionally private and has no effect unless explicitly requested.
if [[ -n ${_RUDDY_INSTALL_STD_TEST_BARRIER:-} ]]; then
  : >"$_RUDDY_INSTALL_STD_TEST_BARRIER"
  while [[ -e $_RUDDY_INSTALL_STD_TEST_BARRIER ]]; do
    sleep 0.01
  done
fi

if [[ $replacement == true ]]; then
  # GNU mv implements this with renameat2(RENAME_EXCHANGE). Unlike moving std
  # aside and then renaming staging, the namespace therefore always contains
  # either complete tree. The capability was exercised above before staging.
  mv --exchange --no-copy -T -- "$staging" "$home/std"
else
  # With no destination, ordinary mv is a same-filesystem directory rename.
  # Avoid GNU-only flags so the first installation remains portable.
  mv "$staging" "$home/std"
fi
printf 'installed Ruddy standard library in %s\n' "$home/std"
