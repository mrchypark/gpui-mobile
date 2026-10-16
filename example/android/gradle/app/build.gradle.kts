// App module build.gradle.kts for the GPUI Mobile Android Example.
//
// This module packages the pre-compiled Rust native library into an APK
// that uses Android's NativeActivity to host the GPUI application.
//
// The Rust library must be compiled separately and placed into
// app/src/main/jniLibs/<abi>/ before building the APK.
//
// Quick start:
//   cd <repo-root>
//   cargo ndk -t arm64-v8a -o example/android_app/gradle/app/src/main/jniLibs \
//       build --example android_app --release
//   cd example/android_app/gradle
//   ./gradlew assembleDebug

plugins {
    id("com.android.application")
}

val bleEnabled = providers.gradleProperty("ble").map(String::toBoolean).getOrElse(false)
val blePackage = rootProject.file("../../../src/packages/ble")
val bleJava = layout.buildDirectory.dir("ble")
val prepareBleJava = tasks.register<Exec>("prepareBleJava") {
    onlyIf { bleEnabled }
    commandLine("sh", blePackage.resolve("prepare-android.sh"), bleJava.get().asFile)
    inputs.file(blePackage.resolve("prepare-android.sh"))
    // Include companion sources and future bridge patch inputs beside the script.
    inputs.files(fileTree(blePackage) { exclude("prepare-android.sh") })
    outputs.dir(bleJava)
}
val buildBleRust = tasks.register<Exec>("buildBleRust") {
    onlyIf { bleEnabled }
    workingDir(rootProject.file("../.."))
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "-o",
        "android/gradle/app/src/main/jniLibs", "build", "--lib", "--features", "ble")
}

tasks.named("preBuild") {
    if (bleEnabled) dependsOn(prepareBleJava, buildBleRust)
}

android {
    namespace = "dev.gpui.mobile.example"
    compileSdk = 34
    buildFeatures { buildConfig = true }

    defaultConfig {
        applicationId = "dev.gpui.mobile.example"
        minSdk = 26          // Vulkan 1.0 is mandatory from API 26+
        targetSdk = 34
        versionCode = 1
        versionName = "1.0.0"
        manifestPlaceholders["mapsApiKey"] = providers.gradleProperty("mapsApiKey")
            .orElse(providers.environmentVariable("MAPS_API_KEY"))
            .getOrElse("")
        buildConfigField("boolean", "BLE_ENABLED", bleEnabled.toString())

        // Tell NativeActivity which .so to load.
        // This must match the cdylib / example output name.
        ndk {
            abiFilters += listOf("arm64-v8a")
        }

        // Forward the library name to the manifest via a placeholder.
        manifestPlaceholders["nativeLibraryName"] = "gpui_mobile_example"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
        debug {
            isDebuggable = true
            isJniDebuggable = true
        }
    }

    // We do NOT use CMake / ndk-build — the native library is compiled
    // externally via cargo-ndk and placed directly into jniLibs.
    //
    // Disable the built-in native build system so Gradle doesn't look for
    // a CMakeLists.txt or Android.mk.
    externalNativeBuild {
        // Intentionally left empty.
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }

    // Tell Gradle where the pre-built .so files live.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
            java.srcDir(blePackage.resolve("java"))
            if (bleEnabled) {
                java.srcDir(bleJava.get().dir("btleplug-0.13.5/src/droidplug/java/src/main/java").asFile)
            }
        }
    }

    packaging {
        // Prevent stripping of the Rust library — cargo already strips in
        // release mode and stripping again can break backtraces.
        jniLibs {
            keepDebugSymbols += listOf(
                "*/arm64-v8a/libgpui_mobile_example.so",
                "*/armeabi-v7a/libgpui_mobile_example.so",
                "*/x86_64/libgpui_mobile_example.so",
                "*/x86/libgpui_mobile_example.so"
            )
        }
    }

    // Lint configuration — relaxed for an example project.
    lint {
        abortOnError = false
        checkReleaseBuilds = false
    }
}

dependencies {
    implementation("com.google.android.gms:play-services-location:21.4.0")
    implementation("com.google.android.gms:play-services-maps:20.0.0")
    implementation("com.google.android.play:review:2.0.2")
    // AndroidX core for NotificationCompat (used by GpuiNotifications)
    implementation("androidx.core:core:1.12.0")
    // AndroidX SplashScreen compat (used by GpuiActivity to hold splash until native init)
    implementation("androidx.core:core-splashscreen:1.0.1")
    // AndroidX Biometric for BiometricPrompt (used by GpuiAuthActivity)
    implementation("androidx.biometric:biometric:1.1.0")
    // AndroidX Media for MediaSessionCompat (used by GpuiMediaSession for system controls)
    implementation("androidx.media:media:1.7.1")
}
