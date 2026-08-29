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
probe_assigned=false
staging=
staging_assigned=false
old_tree=
cleanup() {
  status=$?
  # EXIT traps inherit errexit. Disable it after saving the command status so
  # one failed best-effort deletion cannot prevent the owned-lock release.
  trap - EXIT
  set +e
  cleanup_failed=false

  if [[ -n $staging ]] && ! rm -rf -- "$staging"; then
    printf 'warning: could not remove uninstalled standard-library tree %q\n' "$staging" >&2
    cleanup_failed=true
  fi
  if [[ -n $old_tree ]] && ! rm -rf -- "$old_tree"; then
    printf 'warning: installed the new standard library but could not remove previous tree %q; remove it manually\n' "$old_tree" >&2
    cleanup_failed=true
  fi
  if [[ -n $probe ]] && ! rm -rf -- "$probe"; then
    printf 'warning: could not remove standard-library exchange probe %q\n' "$probe" >&2
    cleanup_failed=true
  fi
  # Process-tagged templates cover a signal after mktemp creates a directory
  # but before command substitution assigns its path. Do not retry assigned
  # paths here: their specific diagnostics above must accurately report whether
  # an old tree still needs manual removal.
  if [[ $probe_assigned == false ]] && ! rm -rf -- "$home/.std.exchange.$temp_tag".*; then
    echo 'warning: could not remove an unassigned standard-library exchange probe' >&2
    cleanup_failed=true
  fi
  if [[ $staging_assigned == false ]] && ! rm -rf -- "$home/.std.install.$temp_tag".*; then
    echo 'warning: could not remove an unassigned standard-library installer tree' >&2
    cleanup_failed=true
  fi
  if [[ $lock_owned == true ]]; then
    # Attempt both operations independently. In particular, failure to remove
    # a temporary/old tree or owner file must never skip the final rmdir.
    if ! rm -f -- "$lock/owner"; then
      printf 'warning: could not remove standard-library installer lock owner %q\n' "$lock/owner" >&2
      cleanup_failed=true
    fi
    if ! rmdir -- "$lock"; then
      printf 'warning: could not release standard-library installer lock %q; remove it manually after verifying no installer is running\n' "$lock" >&2
      cleanup_failed=true
    fi
  fi

  # Preserve an original failure (including signal-derived statuses), but make
  # an otherwise successful install fail when its promised cleanup did not.
  if ((status == 0)) && [[ $cleanup_failed == true ]]; then
    status=1
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# A first install and a concurrent first install must not both decide that std
# is absent: the second `mv staging std` would otherwise put staging *inside*
# std. mkdir is the portable atomic claim. Never reclaim a lock automatically:
# PID checks and pathname removal cannot prove that the directory is still the
# object that was inspected, and can therefore remove a newer installer's lock.
lock_attempts=${_RUDDY_INSTALL_STD_TEST_LOCK_ATTEMPTS:-1000}
if [[ ! $lock_attempts =~ ^[1-9][0-9]*$ ]]; then
  echo '_RUDDY_INSTALL_STD_TEST_LOCK_ATTEMPTS must be a positive integer' >&2
  exit 1
fi
for ((attempt = 1; ; attempt++)); do
  if mkdir -- "$lock" 2>/dev/null; then
    lock_owned=true
    printf 'pid=%s\n' "$$" >"$lock/owner"
    break
  fi
  # Tests use this barrier to replace owner metadata after acquisition failed,
  # exercising the historical observe-then-reap race deterministically.
  if ((attempt == 1)) && [[ -n ${_RUDDY_INSTALL_STD_TEST_LOCK_WAIT_BARRIER:-} ]]; then
    : >"$_RUDDY_INSTALL_STD_TEST_LOCK_WAIT_BARRIER"
    while [[ -e $_RUDDY_INSTALL_STD_TEST_LOCK_WAIT_BARRIER ]]; do
      sleep 0.01
    done
  fi
  if ((attempt >= lock_attempts)); then
    printf 'timed out waiting for standard-library installer lock %q\n' "$lock" >&2
    echo 'another installer may still be running; the lock may also remain after an abnormal exit' >&2
    printf 'after verifying that no installer is running, remove it with: rm -rf -- %q\n' "$lock" >&2
    exit 1
  fi
  sleep 0.01
done

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
  probe_assigned=true
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
staging_assigned=true

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
  old_tree=$staging
  staging=
else
  # With no destination, ordinary mv is a same-filesystem directory rename.
  # Avoid GNU-only flags so the first installation remains portable.
  mv "$staging" "$home/std"
  staging=
fi
printf 'installed Ruddy standard library in %s\n' "$home/std"
