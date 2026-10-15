# Mobile compatibility and validation — 2026-10-15

## Dependency contract

| Component | Exact source / revision |
|---|---|
| Upstream mobile baseline | `https://github.com/itsbalamurali/gpui-mobile`, main `c7cab3a43970bd5f1e05d907695404ed73fcdc95` |
| Fork / branch | `https://github.com/mrchypark/gpui-mobile`, `feature/common-gpui-mobile`; GitHub fork parent and authenticated `mrchypark` account verified |
| GPUI 0.2.2 and gpui_wgpu | `https://github.com/zed-industries/zed`, `1a28cff4b409169bac058bca40dfbfeb7621d19b` |
| wgpu | crates.io `29.0.4`; Android uses the `gpui_wgpu::wgpu` re-export |
| Ely reference | `https://github.com/ZacharyZhang-NY/Ely-GPUI-Components`, `083ef856968812ef83f6dc2db0cc191c50b83cdc` |
| Ely integration | `https://github.com/mrchypark/Ely-GPUI-Components`, `7f6a3f91ceb6e6f6c2dbf4e43d49e01201e384cd` (published mobile support; same GPUI revision) |
| Example zed-reqwest | `https://github.com/zed-industries/reqwest.git`, `33bc764aa15ff7b200bf7c93bd96e24878d53e14` |
| Validation toolchain | Rust `1.97.1`; Apple Silicon; iOS simulator 26.3; Android NDK `28.2.13676358`, API 26 |

The GPUI hash is a Zed revision, not a patch-native commit. The read-only reference was `git show feature/gpui-patch-map:docs/ios-feasibility-20261013.md` plus that branch's manifest/lock. No patch-native or desktop dependency files were changed.

Core gpui-mobile has no Ely dependency. `examples/ely-integration` composes Ely with the mobile platform using the same GPUI instance. `ios::ffi::run_app_with_assets(impl gpui::AssetSource)` accepts application assets without depending on Ely; the app supplies Ely's assets. Both startup paths retain an `ApplicationHandle` for UIKit's external run loop.

Track `example/Cargo.lock` and `examples/ely-integration/Cargo.lock`; the root library lock remains ignored. The tested application locks resolve libc `0.2.189`: libc `0.2.190` plus backtrace `0.3.76` fails on unavailable iOS `_dyld_*` APIs. The direct example reqwest revision matches Zed's reqwest_client; retaining the older `c15662463bda39148ba154100dd44d3fba5873a4` creates two incompatible Client types. TLS certificate validation stays enabled through WebPKI roots.

## Current verification matrix

| Gate | Result | Evidence / boundary |
|---|---|---|
| Common dependency identity | PASS | Root/example iOS and BLE-enabled Android example trees contain one GPUI source/revision and one wgpu |
| GPUI showcase iOS native compile/link/install/run | PASS for the recorded snapshot | iPhone 17 Pro / iOS 26.3, PID 71818 alive at 28 seconds; actual Home rendering visually inspected; `ios-final-build-run.log`, `ios-final-runtime.log`, `ios-gpui-simulator-final.png` |
| Latest iOS Rc/Weak code + Ely native compile/link/install/run | PASS for startup/rendering | Published Ely `7f6a3f91`; final cross-boundary-contact fix; staticlib build 7.33s; PID 20634; `native-ios-reviewed.log`, `runtime-ios-reviewed.log`, `ely-viewport-final.png` |
| Ely adapter regression assertions and actual OS insets | PASS | Debug simulator startup emits `ELY_ADAPTER_SELFCHECK PASS`; real safe area top 62 / bottom 34 reaches live model; source identities in `examples/ely-integration/evidence/` |
| Ely actual click/drag/pinch, keyboard interaction and lifecycle recovery | NOT VERIFIED | Mac locked during UI validation; no UI gestures performed. Simulator developer screenshot confirms rendering only; callback ownership changes are not a full lifecycle stress test |
| Android root default / BLE-enabled type checks | PASS | `android-final-check.log` (58.30s), `android-final-ble-check.log` (1m04s) |
| Android example BLE-enabled type check | PASS | `android-example-final-ble-check.log` (1m00s) |
| Android final BLE-enabled cdylib compile/link | PASS | Latest FIFO/RawOnly/text and Android legacy-flag cleanup; 2026-10-15 20:08 KST, exit 0 in 26.07s; `android-latest-cdylib.log`; source hashes in `android-latest-source-files-final.sha256` |
| Android ELF inspection | PASS | AArch64 DYN; NativeActivity, Insets, BLE and GPS JNI exports; `android-example-elf.txt` |
| BLE-enabled Android debug APK | PASS | Gradle assembleDebug, 1m02s, 36 tasks; pinned BLE Java preparation and Activity compile passed; packaged native library hash matches the final cdylib; `android-latest-apk.log` |
| Android execution | NOT VERIFIED | `adb devices -l` returned no targets; no APK installation or device-run claim |
| Physical GPS/BLE, full surface recovery, FPS | NOT VERIFIED | Type checks and simulator rendering do not establish these results |

Logs are preserved under the calling task's `work/` directory. User-facing artifacts are under its `outputs/` directory. Final Android artifact: `libgpui_mobile_example-android-arm64.so`, SHA-256 `66e37375997042539427fef79516596bcea8888f8659637dc15a8c62d6f5f565`. The build includes source through `1c7c6b3cd5eae08e0a28d4cb563d9e1f9a4b304f`; file hashes and ELF inspection are recorded in `android-latest-source-files-final.sha256` and `android-latest-verification.txt`. Dynamic exports include `ANativeActivity_onCreate`, `Java_dev_gpui_mobile_GpuiInsets_nativeInsets`, `Java_org_gpui_mobile_ble_Ble_initialize`, and `Java_dev_gpui_mobile_GpuiLocation_nativeEvent`.

Independent service/event tests are detailed in [mobile events](mobile-events.md), [location](location-service-integration.md), and [BLE](ble-integration.md). They do not replace device-service validation.

The final debug APK is `outputs/gpui-mobile-example-ble-arm64-debug.apk` (110 MiB), SHA-256 `f9c08f42579fe64352acdfdc157b0f4db75158aafe064b7b571c99ba1aa4da72`. Package `dev.gpui.mobile.example`, minimum API 26, target/compile API 34. APK signature verification passed. Build-Tools 36 used the SDK's existing accepted license; no new license was accepted interactively. Logs include `android-apk-badging.txt`, `android-apk-signature.txt`, and `android-apk-native.sha256`.

## Reproduce

From the repository root, with the corresponding Rust targets installed:

```sh
# Apple SDK tools; boot a simulator first, or set SIMULATOR_UDID.
sh scripts/ios-simulator.sh

# Shared, bounded-size Android build configuration.
export CARGO_TARGET_DIR="$PWD/example/target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=1
ndk_toolchain="$HOME/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin"
export CC_aarch64_linux_android="$ndk_toolchain/aarch64-linux-android26-clang"
export CXX_aarch64_linux_android="$ndk_toolchain/aarch64-linux-android26-clang++"
export AR_aarch64_linux_android="$ndk_toolchain/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
cargo +1.97.1 check --target aarch64-linux-android
cargo +1.97.1 check --features ble --target aarch64-linux-android
cargo +1.97.1 check --locked --manifest-path example/Cargo.toml --lib --features ble --target aarch64-linux-android
cargo +1.97.1 rustc --locked --manifest-path example/Cargo.toml --lib --features ble --crate-type cdylib --target aarch64-linux-android

cargo +1.97.1 tree --locked --manifest-path example/Cargo.toml --features ble --target aarch64-linux-android -i gpui
cargo +1.97.1 tree --locked --manifest-path example/Cargo.toml --features ble --target aarch64-linux-android -i wgpu
"$ndk_toolchain/llvm-readelf" -h -d --dyn-syms "$CARGO_TARGET_DIR/aarch64-linux-android/debug/libgpui_mobile_example.so"

# Package the already verified BLE-enabled native library.
mkdir -p example/android/gradle/app/src/main/jniLibs/arm64-v8a
cp "$CARGO_TARGET_DIR/aarch64-linux-android/debug/libgpui_mobile_example.so" example/android/gradle/app/src/main/jniLibs/arm64-v8a/
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
export ANDROID_HOME="$HOME/Library/Android/sdk" ANDROID_SDK_ROOT="$HOME/Library/Android/sdk"
(cd example/android/gradle && sh ./gradlew --no-daemon --max-workers=1 -Pble=true :app:assembleDebug -x :app:buildBleRust)
```

The Gradle command runs `prepareBleJava`, which invokes `src/packages/ble/prepare-android.sh` and checks the btleplug archive checksum. It excludes only the redundant Rust build because the BLE-enabled cdylib was just compiled and copied above.

Use cdylib-only for the Android example: its general manifest also emits an unnecessary static archive. The dependent gpui-mobile crate still emits its declared archive, so adequate free disk remains necessary. The final command succeeded after disk space was freed; the earlier disk failure is retained in `android-latest-cdylib-disk-full.log`. iOS needs its static library and links with `-force_load` and `-ObjC`; `build.rs` compiles the GPS Objective-C bridge with ARC only for iOS plus the location feature. The simulator script always selects real GPUI rendering, not the host's fallback Metal view.

## Platform limits

- System-power sleep/wake notifications are unsupported here. App background, focus and resume do not synthesize these callbacks; they have separate mobile lifecycle events.
- UIKit idle inhibition is reference-counted and restores the prior idle-timer value on the main queue. Android idle inhibition returns an explicit unsupported error. Pointer visibility/hide behavior remains OS-managed.
- The recorded simulator lacks some AppleColorEmoji glyphs; several example icons render as boxed question marks. Text, quads, cards and navigation render normally. No FPS measurement was taken.
- Renderer creation failures propagate. The Metal display handle and retained embedded app handle fix real black-screen failures that type checking alone did not catch.
