# ProGuard / R8 rules for the GPUI Mobile Android Example.
#
# This is a pure native (Rust) application using NativeActivity — there is no
# Java or Kotlin application code to shrink, optimize, or obfuscate.
#
# These rules are intentionally empty.  They exist only because the
# build.gradle.kts references this file in the release buildType's
# proguardFiles configuration.

# Keep NativeActivity since it is referenced by name in AndroidManifest.xml.
-keep class android.app.NativeActivity { *; }

# Loaded by the Rust platform on the Java UI thread.
-keep class dev.gpui.mobile.GpuiInsets { *; }

# BLE classes are accessed by JNI; preserve names and callbacks.
-keep class org.gpui.mobile.ble.** { *; }
-keep class com.nonpolynomial.** { *; }
-keep class io.github.gedgygedgy.** { *; }
