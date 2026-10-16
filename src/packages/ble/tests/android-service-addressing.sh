#!/bin/sh
# Compile production Java against Android API, then execute only its GATT logic
# with recording Android boundary classes. No device/Gradle/Cargo/radio access.
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ble=$(CDPATH= cd -- "$here/.." && pwd)
: "${ANDROID_HOME:=$HOME/Library/Android/sdk}"
: "${JAVA_HOME:=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home}"
: "${ANDROID_JAR:=$ANDROID_HOME/platforms/android-34/android.jar}"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/gpui-ble-addressing.XXXXXX")
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
find "$ble/vendor/btleplug-0.13.5/src/droidplug/java/src/main/java" "$ble/java" -name '*.java' -print > "$scratch/production.txt"
"$JAVA_HOME/bin/javac" -cp "$ANDROID_JAR" -d "$scratch/production" @"$scratch/production.txt"
find "$here/android" -name '*.java' -print > "$scratch/tests.txt"
"$JAVA_HOME/bin/javac" -cp "$scratch/production:$ANDROID_JAR" -d "$scratch/tests" @"$scratch/tests.txt"
"$JAVA_HOME/bin/java" -ea -cp "$scratch/tests:$scratch/production:$ANDROID_JAR" com.nonpolynomial.btleplug.android.impl.ServiceAddressingTest
