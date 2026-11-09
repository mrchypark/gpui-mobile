#!/bin/sh
# Same Ely UI, packaged with the repository's NativeActivity/JNI host.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/../.." && pwd)
: "${ANDROID_HOME:=$HOME/Library/Android/sdk}"
: "${ANDROID_NDK_HOME:=$ANDROID_HOME/ndk/28.2.13676358}"
if [ -z "${JAVA_HOME:-}" ]; then
    if [ -d /opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home ]; then
        JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
    else
        JAVA_HOME=$(/usr/libexec/java_home -v 21)
    fi
fi
export ANDROID_HOME ANDROID_NDK_HOME JAVA_HOME
ndk_bin="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin"
export CC_aarch64_linux_android="$ndk_bin/aarch64-linux-android26-clang"
export CXX_aarch64_linux_android="$ndk_bin/aarch64-linux-android26-clang++"
export AR_aarch64_linux_android="$ndk_bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/example/target}"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
features=
ble_property=
if [ "${GPUI_MOBILE_BLE:-1}" = 1 ]; then
    features=ble
    ble_property=-Pble=true
fi
if [ "${GPUI_PLATFORM_DEVICE:-0}" = 1 ]; then
    features="${features:+$features,}platform-device"
elif [ "${GPUI_PLATFORM_EXTENDED:-0}" = 1 ]; then
    features="${features:+$features,}platform-extended"
elif [ "${GPUI_PLATFORM_SMOKE:-0}" = 1 ]; then
    features="${features:+$features,}platform-smoke"
fi
feature_arg=
if [ -n "$features" ]; then feature_arg="--features=$features"; fi
# Android 16 KB page devices require every ELF LOAD segment to use 16 KB alignment.
cargo +1.97.1 rustc --manifest-path "$here/Cargo.toml" --locked --lib $feature_arg \
    --target aarch64-linux-android -j 1 --crate-type cdylib -- \
    -C link-arg=-Wl,-z,max-page-size=16384
library="$CARGO_TARGET_DIR/aarch64-linux-android/debug/libgpui_mobile_ely_integration.so"
"$ndk_bin/llvm-readelf" -lW "$library" | awk '
    $1 == "LOAD" { found = 1; if ($NF != "0x4000") invalid = 1 }
    END { exit !found || invalid }
'
mkdir -p "$here/android-build/jniLibs/arm64-v8a"
cp "$library" "$here/android-build/jniLibs/arm64-v8a/"
sh "$repo/example/android/gradle/gradlew" -p "$here/android" --no-daemon --max-workers=1 $ble_property :app:assembleDebug
cp "$here/android/app/build/outputs/apk/debug/app-debug.apk" "$here/android-build/ely-integration-arm64-debug.apk"
apk="$here/android-build/ely-integration-arm64-debug.apk"
zipalign=$(find "$ANDROID_HOME/build-tools" -type f -name zipalign | sort | tail -n 1)
test -n "$zipalign"
"$zipalign" -c -P 16 -v 4 "$apk"
printf '%s\n' "$apk"
