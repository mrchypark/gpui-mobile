#!/bin/sh
# Java and Rust must use exactly the same pinned service-addressing patch.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec sh "$here/prepare-vendor.sh" "$@"
