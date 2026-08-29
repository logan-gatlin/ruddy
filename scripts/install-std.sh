#!/usr/bin/env bash
# Install a source-tree copy of Ruddy's standard library without exposing a
# partially copied project. The destination is replaced only after staging has
# completed on the same filesystem.
set -euo pipefail

source_input=${1:?usage: install-std.sh SOURCE}
if ! source=$(cd -P -- "$source_input" 2>/dev/null && pwd); then
  printf 'standard-library source %q is not a directory\n' "$source_input" >&2
  exit 1
fi
if [[ ! -f "$source/Ruddy.toml" || ! -f "$source/main.hc" ]]; then
  printf 'standard-library source %q is not a Ruddy project\n' "$source_input" >&2
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
lock=$home/.std.install.lock
lock_owned=false
temp_tag=$$.$RANDOM.$RANDOM
probe=
staging=
cleanup() {
  status=$?
  # Before the commit this removes the uninstalled tree. After an exchange,
  # this same path names the previous installation instead.
  [[ -z $staging ]] || rm -rf -- "$staging"
  [[ -z $probe ]] || rm -rf -- "$probe"
  # Process-tagged templates also cover a signal after mktemp creates a directory
  # but before command substitution assigns its path to the variables above.
  rm -rf -- "$home/.std.exchange.$temp_tag".* "$home/.std.install.$temp_tag".*
  if [[ $lock_owned == true ]]; then
    rm -f -- "$lock/owner"
    rmdir -- "$lock" 2>/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# A first install and a concurrent first install must not both decide that std
# is absent: the second `mv staging std` would otherwise put staging *inside*
# std. mkdir is the portable atomic lock primitive. Dead owners are reclaimed
# under a marker inside the old lock; the owner is checked again after claiming
# that marker so a contender cannot remove a newly acquired lock.
while ! mkdir -- "$lock" 2>/dev/null; do
  owner=$(cat "$lock/owner" 2>/dev/null || true)
  if [[ $owner =~ ^[0-9]+$ ]] && ! kill -0 "$owner" 2>/dev/null; then
    if mkdir -- "$lock/reaping" 2>/dev/null; then
      current=$(cat "$lock/owner" 2>/dev/null || true)
      if [[ $current == "$owner" ]] && ! kill -0 "$current" 2>/dev/null; then
        rm -rf -- "$lock"
      else
        rmdir -- "$lock/reaping" 2>/dev/null || true
      fi
    fi
  fi
  sleep 0.01
done
lock_owned=true
printf '%s\n' "$$" >"$lock/owner"

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
  probe=$(mktemp -d "$home/.std.exchange.$temp_tag.XXXXXX")
  mkdir "$probe/left" "$probe/right"
  if ! mv --exchange --no-copy -T -- "$probe/left" "$probe/right"; then
    echo 'cannot atomically replace the Ruddy standard library: this Linux filesystem does not support directory exchange' >&2
    echo 'remove the existing std directory to perform a portable first install' >&2
    exit 1
  fi
  rm -rf -- "$probe"
  probe=
fi

staging=$(mktemp -d "$home/.std.install.$temp_tag.XXXXXX")

# Copy only the manifest and Ruddy source tree. In particular, build products,
# repository metadata, and editor files never become part of the installation.
# Discover into a regular file first: bash does not propagate a process
# substitution's status, so `while ... < <(find ...)` could commit the files
# emitted before a failed find and silently replace a complete installation.
# Running find from the resolved source root also makes ./build a literal path,
# even when the caller's path contains find pattern metacharacters.
source_files=$staging/.source-files
(cd -- "$source" && find . -path ./build -prune -o -type f -name '*.hc' -print0) >"$source_files"
cp -- "$source/Ruddy.toml" "$staging/Ruddy.toml"
while IFS= read -r -d '' file; do
  relative=${file#./}
  destination=$staging/$relative
  mkdir -p -- "${destination%/*}"
  cp -- "$source/$relative" "$destination"
done <"$source_files"
rm -- "$source_files"

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
