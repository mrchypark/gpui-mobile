#!/bin/sh
# Reproduce the checked-in crate and matching Java ABI from one verified release.
set -eu
test "$#" -eq 1 || { echo 'usage: prepare-vendor.sh OUTPUT_DIRECTORY' >&2; exit 2; }
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
mkdir -p "$1"
output=$(CDPATH= cd -- "$1" && pwd)
download=
archive=${BTLEPLUG_ARCHIVE:-}
if test -z "$archive"; then
    download=$(mktemp "${TMPDIR:-/tmp}/gpui-btleplug.XXXXXX")
    trap 'rm -f "$download"' EXIT HUP INT TERM
    archive=$download
    curl --fail --location --silent --show-error --retry 2 \
        https://crates.io/api/v1/crates/btleplug/0.13.5/download -o "$archive"
fi
printf '%s  %s\n' 3d836502de731602ea87cbb307065e8feaed4fd10f0855a75b0b87944ee011dc "$archive" | shasum -a 256 -c -
tar -xzf "$archive" -C "$output"
patch -s -p1 -d "$output/btleplug-0.13.5" < "$here/patches/android-service-addressing.patch"
patch -s -p1 -d "$output/btleplug-0.13.5" < "$here/patches/android-idle-disconnect.patch"
patch -s -p1 -d "$output/btleplug-0.13.5" < "$here/patches/android-abort-disconnect.patch"
# Detect drift between reproducible preparation and the checked-in bridge.
cmp "$output/btleplug-0.13.5/src/droidplug/java/src/main/java/com/nonpolynomial/btleplug/android/impl/Peripheral.java" \
    "$here/vendor/btleplug-0.13.5/src/droidplug/java/src/main/java/com/nonpolynomial/btleplug/android/impl/Peripheral.java"
