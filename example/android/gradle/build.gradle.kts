// Root build.gradle.kts for the GPUI Mobile Android Example project.
//
// This is a minimal Gradle project that packages the Rust native library
// (compiled separately via cargo-ndk) into an APK using NativeActivity.
//
// Build steps:
//   1. Compile the Rust library from `example/`:
//      cargo ndk -t arm64-v8a -o android/gradle/app/build/generated/jniLibs/debug build --lib
//
//   2. Build the APK:
//      ./gradlew assembleDebug
//
//   3. Install on device/emulator:
//      adb install app/build/outputs/apk/debug/app-debug.apk

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.2.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.3.21")
    }
}

tasks.register("clean", Delete::class) {
    delete(rootProject.layout.buildDirectory)
}

// ── Convenience task: build Rust + APK in one go ────────────────────────────

tasks.register<Exec>("buildRustRelease") {
    group = "rust"
    description = "Compile the Rust native library for arm64-v8a using cargo-ndk."
    workingDir = rootProject.projectDir.parentFile.parentFile // -> example/
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-o", "android/gradle/app/build/generated/jniLibs/release",
        "build", "--lib", "--release"
    )
}

tasks.register<Exec>("buildRustDebug") {
    group = "rust"
    description = "Compile the Rust native library for arm64-v8a (debug) using cargo-ndk."
    workingDir = rootProject.projectDir.parentFile.parentFile
    commandLine(
        "cargo", "ndk",
        "-t", "arm64-v8a",
        "-o", "android/gradle/app/build/generated/jniLibs/debug",
        "build", "--lib"
    )
}

tasks.register("buildAll") {
    group = "rust"
    description = "Build the release Rust library and assemble the release APK."
    if (!providers.gradleProperty("ble").map(String::toBoolean).getOrElse(false)) {
        dependsOn("buildRustRelease")
    }
    dependsOn(":app:assembleRelease")
}
