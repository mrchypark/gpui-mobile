fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        // Platform file preview is not package-feature gated.
        println!("cargo:rustc-link-lib=framework=QuickLook");
        if std::env::var_os("CARGO_FEATURE_IMAGE_PICKER").is_some() {
            println!("cargo:rustc-link-lib=framework=ImageIO");
        }
        if std::env::var_os("CARGO_FEATURE_FILE_SELECTOR").is_some() {
            println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
        }
        if std::env::var_os("CARGO_FEATURE_PERMISSION_HANDLER").is_some() {
            println!("cargo:rustc-link-lib=framework=Speech");
            println!("cargo:rustc-link-lib=framework=AppTrackingTransparency");
        }
    }
    println!("cargo:rerun-if-changed=src/packages/location/GpuiLocation.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios")
        && std::env::var_os("CARGO_FEATURE_LOCATION").is_some()
    {
        cc::Build::new()
            .file("src/packages/location/GpuiLocation.m")
            .flag("-fobjc-arc")
            .compile("gpui_location");
        println!("cargo:rustc-link-lib=framework=CoreLocation");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=UIKit");
    }
    println!("cargo:rerun-if-changed=src/packages/sensors/GpuiSensors.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios")
        && std::env::var_os("CARGO_FEATURE_SENSORS").is_some()
    {
        cc::Build::new()
            .file("src/packages/sensors/GpuiSensors.m")
            .flag("-fobjc-arc")
            .compile("gpui_sensors");
        println!("cargo:rustc-link-lib=framework=CoreMotion");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=UIKit");
    }
    println!("cargo:rerun-if-changed=src/packages/media_session/GpuiMediaSession.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios")
        && std::env::var_os("CARGO_FEATURE_MEDIA_SESSION").is_some()
    {
        cc::Build::new()
            .file("src/packages/media_session/GpuiMediaSession.m")
            .flag("-fobjc-arc")
            .compile("gpui_media_session");
        println!("cargo:rustc-link-lib=framework=MediaPlayer");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
