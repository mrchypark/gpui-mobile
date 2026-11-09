#!/bin/sh
# Isolated framework regression APK; builds only. Parent owns install/run/remove.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo=$(CDPATH= cd -- "$here/../.." && pwd)
: "${ANDROID_HOME:=$HOME/Library/Android/sdk}"
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home}"
: "${ANDROID_NDK_HOME:=$ANDROID_HOME/ndk/28.2.13676358}"
: "${ANDROID_BUILD_TOOLS:=$ANDROID_HOME/build-tools/34.0.0}"
test "$#" -eq 1 || { echo 'usage: build.sh NEW_OUTPUT_DIRECTORY' >&2; exit 2; }
mkdir "$1"
out=$(CDPATH= cd -- "$1" && pwd)
mkdir -p "$out/classes" "$out/dex" "$out/apk/lib/arm64-v8a"
production=${KEYBOARD_SOURCE:-$repo/example/android/gradle/app/src/main/java/dev/gpui/mobile/GpuiKeyboard.java}
shasum -a 256 "$production" "$here/KeyboardRegression.java" "$here/bridge.c" "$here/AndroidManifest.xml" > "$out/source.sha256"
"$JAVA_HOME/bin/javac" -source 8 -target 8 -encoding UTF-8 -classpath "$ANDROID_HOME/platforms/android-34/android.jar" \
  -d "$out/classes" "$production" "$here/KeyboardRegression.java"
"$JAVA_HOME/bin/jar" cf "$out/classes.jar" -C "$out/classes" .
JAVA_HOME="$JAVA_HOME" "$ANDROID_BUILD_TOOLS/d8" --min-api 26 --lib "$ANDROID_HOME/platforms/android-34/android.jar" \
  --output "$out/dex" "$out/classes.jar"
"$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android26-clang" \
  -shared -fPIC "$here/bridge.c" -o "$out/apk/lib/arm64-v8a/libgpui_keyboard_regression.so"
"$ANDROID_BUILD_TOOLS/aapt" package -f -M "$here/AndroidManifest.xml" \
  -I "$ANDROID_HOME/platforms/android-34/android.jar" -F "$out/unsigned.apk"
cp "$out/dex/classes.dex" "$out/apk/classes.dex"
(cd "$out/apk" && zip -q "$out/unsigned.apk" classes.dex lib/arm64-v8a/libgpui_keyboard_regression.so)
"$ANDROID_BUILD_TOOLS/zipalign" -f 4 "$out/unsigned.apk" "$out/aligned.apk"
"$JAVA_HOME/bin/keytool" -genkeypair -keystore "$out/test-only.jks" -storepass android -keypass android \
  -alias test -dname CN=KeyboardRegression -keyalg RSA -validity 30 -noprompt
JAVA_HOME="$JAVA_HOME" "$ANDROID_BUILD_TOOLS/apksigner" sign --ks "$out/test-only.jks" --ks-pass pass:android \
  --out "$out/keyboard-regression.apk" "$out/aligned.apk"
shasum -a 256 -c "$out/source.sha256"
