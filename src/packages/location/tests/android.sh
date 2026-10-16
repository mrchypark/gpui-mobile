#!/bin/sh
# No dependency downloads: provide a classpath exported by the resolved Android host.
set -eu
location_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
repo=$(CDPATH= cd -- "$location_dir/../../.." && pwd)
if [ -z "${LOCATION_JAVA_CLASSPATH_FILE:-}" ] || [ ! -r "$LOCATION_JAVA_CLASSPATH_FILE" ]; then
    echo 'Set LOCATION_JAVA_CLASSPATH_FILE to a readable resolved host javac classpath file (android.jar + GMS location/base/basement/tasks and transitive jars). This test never downloads dependencies.' >&2
    exit 2
fi
classpath=$(cat "$LOCATION_JAVA_CLASSPATH_FILE")
if [ -z "$classpath" ]; then echo 'Resolved Java classpath is empty' >&2; exit 2; fi
old_ifs=$IFS
IFS=:
set -f
for entry in $classpath; do
    if [ ! -f "$entry" ]; then echo 'Resolved Java classpath contains a missing/non-file entry; regenerate the host classpath.' >&2; exit 2; fi
done
set +f
IFS=$old_ifs
javac=${JAVA_HOME:+$JAVA_HOME/bin/}javac
java=${JAVA_HOME:+$JAVA_HOME/bin/}java
command -v "$javac" >/dev/null
command -v "$java" >/dev/null
scratch=$(mktemp -d "${TMPDIR:-/tmp}/gpui-location-java.XXXXXX")
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
source_dir="$repo/example/android/gradle/app/src/main/java/dev/gpui/mobile"
"$javac" -J-Xmx256m -cp "$classpath" -d "$scratch" \
    "$source_dir/GpuiLocation.java" "$source_dir/GpuiGmsLocation.java" "$source_dir/GpuiLocationService.java" \
    "$location_dir/tests/AndroidFixTest.java" "$location_dir/tests/AndroidServiceLifecycleTest.java"
"$java" -ea -cp "$scratch:$classpath" dev.gpui.mobile.AndroidFixTest
"$java" -ea -cp "$scratch:$classpath" dev.gpui.mobile.AndroidServiceLifecycleTest
sh "$location_dir/tests/gms-fixture/run.sh"
