#!/bin/sh
# Build the real example with Apple's SDK tools; no XcodeGen or signing account required.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root/example"
export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-1.97.1}
export IPHONEOS_DEPLOYMENT_TARGET=16.0
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_INCREMENTAL=0
cargo build --locked --lib --target aarch64-apple-ios-sim
sdk=$(xcrun --sdk iphonesimulator --show-sdk-path)
app="$root/example/ios/build/GpuiExample.app"
mkdir -p "$app"
cp ios/Info.plist "$app/Info.plist"
xcrun ibtool --compile "$app/LaunchScreen.storyboardc" ios/LaunchScreen.storyboard --target-device iphone --target-device ipad --minimum-deployment-target 16.0
xcrun --sdk iphonesimulator clang -target arm64-apple-ios16.0-simulator -isysroot "$sdk" \
  -fobjc-arc -DUSE_GPUI_RUST=1 -Iios ios/main.m \
  -Wl,-force_load,"$root/example/target/aarch64-apple-ios-sim/debug/libgpui_mobile_example.a" \
  -lc++ -liconv -lresolv -lz -framework UIKit -framework Metal -framework MetalKit \
  -framework QuartzCore -framework CoreGraphics -framework CoreText -framework CoreFoundation \
  -framework Foundation -framework Security -framework SystemConfiguration -framework AudioToolbox \
  -framework WebKit -framework CoreMedia -framework AVFoundation -framework LocalAuthentication \
  -framework Contacts -framework EventKit -framework StoreKit -framework CoreMotion \
  -framework CoreLocation -framework Photos -framework PhotosUI -framework CoreVideo \
  -framework CoreBluetooth -ObjC -o "$app/GpuiExample"
codesign --force --sign - "$app"
if [ "${1:-}" != "--no-run" ]; then
  simulator=${SIMULATOR_UDID:-booted}
  xcrun simctl install "$simulator" "$app"
  xcrun simctl launch "$simulator" dev.gpui.mobile.example
fi
printf '%s\n' "$app"
