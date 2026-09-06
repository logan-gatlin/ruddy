#!/usr/bin/env bash
# Cargo invokes this only after compilation, with the test executable and args.
set -euo pipefail

if systemd-run --user --scope --quiet true 2>/dev/null; then
    # The limit includes every thread and child process of this test executable.
    # Zero swap keeps the bound at 4 GiB rather than 4 GiB plus swap.
    exec systemd-run --user --scope --quiet \
        --property=MemoryMax=4G \
        --property=MemorySwapMax=0 \
        --property=OOMPolicy=continue \
        timeout --signal=TERM --kill-after=30s 30m "$@"
fi

# No systemd user bus — a container or another OS. Preserve the portable fallback.
exec "$@"
