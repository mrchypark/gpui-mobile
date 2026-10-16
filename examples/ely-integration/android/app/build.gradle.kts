plugins { id("com.android.application") }
val repo = rootProject.file("../../..")
val host = repo.resolve("example/android/gradle/app/src/main")
android {
    // Existing host Java imports this BuildConfig; the installed app ID is distinct.
    namespace = "dev.gpui.mobile.example"
    compileSdk = 34
    buildFeatures { buildConfig = true }
    defaultConfig {
        applicationId = "dev.gpui.mobile.ely"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "1.0.0"
        manifestPlaceholders["mapsApiKey"] = providers.gradleProperty("mapsApiKey")
            .orElse(providers.environmentVariable("MAPS_API_KEY"))
            .getOrElse("")
        buildConfigField("boolean", "BLE_ENABLED", "false")
        ndk { abiFilters += "arm64-v8a" }
    }
    buildTypes { debug { isDebuggable = true; isJniDebuggable = true } }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    sourceSets.getByName("main") {
        java.srcDirs(host.resolve("java"), repo.resolve("src/packages/ble/java"))
        res.srcDir(host.resolve("res"))
        assets.srcDir(host.resolve("assets"))
        jniLibs.srcDir(rootProject.file("../android-build/jniLibs"))
    }
    packaging.jniLibs.keepDebugSymbols += "**/libgpui_mobile_ely_integration.so"
}
dependencies {
    implementation("com.google.android.gms:play-services-location:21.4.0")
    implementation("com.google.android.gms:play-services-maps:20.0.0")
    implementation("com.google.android.play:review:2.0.2")
    implementation("androidx.core:core:1.12.0")
    implementation("androidx.core:core-splashscreen:1.0.1")
    implementation("androidx.biometric:biometric:1.1.0")
    implementation("androidx.media:media:1.7.1")
}
