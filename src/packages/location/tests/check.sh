#!/bin/sh
# Service-only checks, independent of GPUI and its renderer dependencies.
set -eu
location_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
case "${1:-all}" in
    --android-only) exec sh "$location_dir/tests/android.sh" ;;
    all) sh "$location_dir/tests/android.sh" ;;
    *) echo 'Usage: check.sh [--android-only]' >&2; exit 2 ;;
esac
scratch=$(mktemp -d /private/tmp/gpui-location-check.XXXXXX)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
ln -s "$location_dir" "$scratch/location"
mkdir "$scratch/src"
cat > "$scratch/Cargo.toml" <<'TOML'
[package]
name = "gpui-location-service-check"
version = "0.0.0"
edition = "2021"
[dependencies]
futures = "0.3"
TOML
cat > "$scratch/src/lib.rs" <<'RUST'
#[path = "../location/mod.rs"]
pub mod location;
RUST
cargo test --offline --manifest-path "$scratch/Cargo.toml"
if command -v xcrun >/dev/null 2>&1; then
    sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
    xcrun --sdk iphonesimulator clang -fobjc-arc -Wall -Wextra -Werror -Wno-unused-parameter \
        -target arm64-apple-ios16.0-simulator -isysroot "$sdk" \
        -framework UIKit -framework Foundation -framework CoreLocation \
        "$location_dir/tests/ios_callbacks.m" -o "$scratch/ios-callback-tests"
    if test -n "${LOCATION_SIMULATOR_UDID:-}"; then
        xcrun simctl spawn "$LOCATION_SIMULATOR_UDID" "$scratch/ios-callback-tests"
    fi
fi
