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
cargo +1.97.1 rustc --manifest-path "$here/Cargo.toml" --locked --lib \
    --target aarch64-linux-android -j 1 --crate-type cdylib
mkdir -p "$here/android-build/jniLibs/arm64-v8a"
cp "$CARGO_TARGET_DIR/aarch64-linux-android/debug/libgpui_mobile_ely_integration.so" \
    "$here/android-build/jniLibs/arm64-v8a/"
sh "$repo/example/android/gradle/gradlew" -p "$here/android" --no-daemon --max-workers=1 :app:assembleDebug
cp "$here/android/app/build/outputs/apk/debug/app-debug.apk" "$here/android-build/ely-integration-arm64-debug.apk"
printf '%s\n' "$here/android-build/ely-integration-arm64-debug.apk"
