#!/bin/sh
# Reuse the real UIKit host; only its startup symbol and bundle identity differ.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-1.97.1}
export IPHONEOS_DEPLOYMENT_TARGET=16.0
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-"$root/example/target"}
cargo rustc --crate-type staticlib --manifest-path "$here/Cargo.toml" --locked --lib --target aarch64-apple-ios-sim -j 1
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
host="$root/example/ios"
app="$here/ios-build/ElyIntegration.app"
mkdir -p "$app"
cp "$host/Info.plist" "$app/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleIdentifier dev.gpui.mobile.ely' "$app/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleExecutable ElyIntegration' "$app/Info.plist"
/usr/libexec/PlistBuddy -c 'Set :CFBundleName Ely Integration' "$app/Info.plist"
xcrun ibtool --compile "$app/LaunchScreen.storyboardc" "$host/LaunchScreen.storyboard" --target-device iphone --target-device ipad --minimum-deployment-target 16.0
xcrun --sdk iphonesimulator clang -target arm64-apple-ios16.0-simulator -isysroot "$sdk" \
  -fobjc-arc -DUSE_GPUI_RUST=1 -Dgpui_ios_run_demo=gpui_ios_run_ely -I"$host" "$host/main.m" \
  -Wl,-force_load,"$CARGO_TARGET_DIR/aarch64-apple-ios-sim/debug/libgpui_mobile_ely_integration.a" \
  -lc++ -liconv -lresolv -lz -framework UIKit -framework Metal -framework MetalKit \
  -framework QuartzCore -framework CoreGraphics -framework CoreText -framework CoreFoundation \
  -framework Foundation -framework Security -framework SystemConfiguration -framework AudioToolbox \
  -framework WebKit -framework CoreMedia -framework AVFoundation -framework LocalAuthentication \
  -framework Contacts -framework EventKit -framework StoreKit -framework CoreMotion \
  -framework CoreLocation -framework Photos -framework PhotosUI -framework CoreVideo \
  -framework CoreBluetooth -ObjC -o "$app/ElyIntegration"
codesign --force --sign - "$app"
if [ "${1:-}" != "--no-run" ]; then
  simulator=${SIMULATOR_UDID:-booted}
  xcrun simctl install "$simulator" "$app"
  xcrun simctl launch "$simulator" dev.gpui.mobile.ely
fi
printf '%s\n' "$app"
