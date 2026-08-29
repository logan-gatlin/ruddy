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

if [[ -e $home/std || -L $home/std ]]; then
  # GNU mv implements this with renameat2(RENAME_EXCHANGE) on Linux. Unlike
  # moving std aside and then renaming staging, the namespace therefore always
  # contains either the complete old tree or the complete new tree. --no-copy
  # forbids a non-atomic fallback, and -T makes both operands the trees to swap.
  mv --exchange --no-copy -T -- "$staging" "$home/std"
else
  mv --no-copy -T -- "$staging" "$home/std"
fi
printf 'installed Ruddy standard library in %s\n' "$home/std"
