fn main() {
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
}
