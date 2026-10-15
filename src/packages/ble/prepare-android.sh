#!/bin/sh
# Fetch the Java half of the exact Rust crate, verified against crates.io's checksum.
set -eu
if [ "$#" -ne 1 ]; then
    echo 'usage: prepare-android.sh OUTPUT_DIRECTORY' >&2
    exit 2
fi
mkdir -p "$1"
ble_output=$(cd "$1" && pwd)
ble_archive=$(mktemp "${TMPDIR:-/tmp}/gpui-ble.XXXXXX")
trap 'rm -f "$ble_archive"' EXIT HUP INT TERM
curl --fail --location --silent --show-error --retry 2 \
    https://crates.io/api/v1/crates/btleplug/0.13.5/download -o "$ble_archive"
printf '%s  %s\n' 3d836502de731602ea87cbb307065e8feaed4fd10f0855a75b0b87944ee011dc "$ble_archive" | shasum -a 256 -c -
tar -xzf "$ble_archive" -C "$ble_output"
test -f "$ble_output/btleplug-0.13.5/src/droidplug/java/src/main/java/com/nonpolynomial/btleplug/android/impl/Adapter.java"
