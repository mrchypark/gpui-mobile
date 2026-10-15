# Native Ely mobile viewport

A small native app using actual Ely Button/Input/InfiniteCanvas components,
mobile OS events, and one GPUI dependency graph. No network client or TLS bypass.

## Build and run

From the repository root on an Apple Silicon Mac with Xcode and a booted iOS
simulator:

```sh
rustup target add --toolchain 1.97.1 aarch64-apple-ios-sim
sh examples/ely-integration/ios-simulator.sh
```

The POSIX script uses the existing `example/ios/main.m`, header and storyboard;
no copied host or fallback renderer. It builds only the app staticlib with one
job, debug=0 and incremental=false, then links, signs ad hoc, installs and
launches `dev.gpui.mobile.ely`. `--no-run` packages without installing/launching.
`SIMULATOR_UDID` selects a device; `CARGO_TARGET_DIR` defaults to `example/target`.
For an already populated cache, `CARGO_NET_OFFLINE=true` is optional. The script
uses `--locked` but does not require offline caches on fresh clones.

Generated `ios-build/` is ignored. Do not commit the app or build artifacts.

### Android APK

On macOS, install Android SDK platform 34, NDK 28.2.13676358, JDK 21 and the
Rust 1.97.1 `aarch64-linux-android` target, then run:

```sh
sh examples/ely-integration/android-apk.sh
```

If the shell already exports an older JDK, explicitly set `JAVA_HOME` to JDK 21
before running the script (for Homebrew: `/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home`).

`ANDROID_HOME`, `ANDROID_NDK_HOME`, `JAVA_HOME` and `CARGO_TARGET_DIR` can override
the local defaults. The script builds only an arm64 cdylib with `--locked`, one
job, debug=0 and incremental=false, then uses the existing Gradle wrapper to
assemble a signed debug APK. It reuses the repository's NativeActivity Java,
resources and assets without copying or changing that host. Generated staging
and Gradle outputs are ignored.

The distinct output is `android-build/ely-integration-arm64-debug.apk` beneath
this example, with application ID `dev.gpui.mobile.ely`, label `Ely Integration`,
and native library `gpui_mobile_ely_integration`. This is the Ely UI, not the
BLE demo. The manifest requests no network or BLE permissions. The reused host's
BLE initialization is disabled.

The final arm64 cdylib and debug APK built successfully on 2026-10-15. Manifest,
NativeActivity/JNI exports and APK v2 debug signature verification passed; see
`evidence/native-android.txt` for hashes and commands. AndroidX adds biometric,
fingerprint and internal receiver permissions through manifest merging.

## Source identity

- Ely: published `https://github.com/mrchypark/Ely-GPUI-Components`,
  `7f6a3f91ceb6e6f6c2dbf4e43d49e01201e384cd`.
- GPUI and gpui_wgpu: Zed `1a28cff4b409169bac058bca40dfbfeb7621d19b`.
- wgpu: crates.io `29.0.4`; no gpui-pre packages.

`evidence/source-identity*.json` records actual Cargo metadata package IDs and
full sources for both iOS and Android. Exactly one of each package was found;
the identities match between targets. `evidence/*-ios.txt` and
`evidence/gpui-android.txt` are active locked dependency trees.

This is an independent workspace/lockfile. The app root repeats the existing
async-task patch because dependency manifest patches are ignored. The lock
uses libc 0.2.189 to avoid the libc 0.2.190/backtrace iOS `_dyld_*` regression.
No root/desktop lock change is required.

## Actual event wiring

The native observer selects policy synchronously before backend synthesis,
without borrowing GPUI App or pumping nested events. It queues model updates
on the UI executor. The owning native window ID is explicitly captured; foreign
windows select legacy delivery and cannot feed this canvas.

The first Started hit-tests **paint-measured** canvas bounds. Inside selects
RawOnly; outside selects LegacyCompatible for real Button/Input/scroll handling.
All extra fingers inherit that route even outside the start region. Ended and
Cancelled leave the policy latched through the backend's synthesis check; the
next Started selects again. Policy is thread-wide, so this observer restores
the appropriate decision for each native delivery.

The raw adapter subtracts the painted window origin once and sends full logical
contact snapshots to Ely CanvasTouch. Ended's final position is supplied before
removal. A second-finger TouchAction::Cancel rolls back only the editing preview;
it retains contacts and the pinch recognizer. OS cancel, pause and surface loss
clear the sequence. CanvasTouch alone recognizes pinch; cumulative backend
MobileEvent::Pinch is ignored, preventing double zoom. RawOnly suppresses legacy
touch-generated mouse/scroll in the canvas; outside controls retain synthesis.

Safe area and docked bottom keyboard occlusion combine using max, not addition.
A keyboard rectangle must reach the bottom **and cover the usable content width**
to count as bottom occlusion. Floating rectangles remain in WindowInsets. The
focused input's actual painted bounds are checked against keyboard geometry on
focus/keyboard/frame changes; Ely reveal helpers scroll it only when those bounds
intersect the keyboard. Unchanged paints and hidden-keyboard updates preserve
manual scrolling. Ordinary offscreen fields do not trigger keyboard avoidance.
Full control-area occlusion has no vertical scroll destination and remains a
limitation. Android IME bottom insets also
participate in the max. The header scroll area and input remain actual Ely UI.

## Startup and evidence

`gpui_ios_register_app` registers the root. The host's startup symbol maps to
`gpui_ios_run_ely`, which supplies Assets through generic
`ios::ffi::run_app_with_assets`. That API is Ely-free and preserves FFI setup,
current_platform, run_embedded, retained ApplicationHandle and lifecycle dispatch.
The core crate has no Ely dependency.

The final published-revision app compiled (7.33s), linked, signed and launched
in the iOS simulator on 2026-10-15 at 20:01 KST, PID 20634. This includes the
cross-boundary additional-finger fix and current Rc/Weak native ownership code.
Its debug startup adapter assertions passed, including final Ended coordinates
and a canvas-start/header-second-finger pinch. Actual OS safe-area events (top
62, bottom 34 logical pixels) reached the live view. The developer screenshot
shows actual Ely Button/Input/canvas; see `evidence/native-ios.txt`.

The final Android-packaging/keyboard source was also built and developer-launched
on iOS at 21:28 KST (PID 18809). The 3m56s build, native link and signing passed.
Both `ELY_FLOATING_KEYBOARD_SELFCHECK PASS` and `ELY_ADAPTER_SELFCHECK PASS`
executed using real GPUI types and Ely helpers. This covers focus, keyboard
movement/resizing/hiding, manual-scroll preservation, and the nonintersecting
offscreen-field regression. The new `ely-viewport-keyboard-ready.png` developer
screenshot confirms the initial button, input, status and canvas remain visible.

Later unlocked UI verification tested the local iOS focus-hook fix on top of
commit 7253295 (4m15s build/link/sign, PID 11799). Software-keyboard AX clicks
entered exact `ely`; Simulator toggle show/hide delivered keyboard Insets and
preserved text. Outside button reset, canvas tap/drag (pan -75,-50, commits 2),
and Home/icon resume passed with text and viewport preserved. See
`docs/ios-ui-verification.md` at the repository root for traces, screenshots and
exact source/build hashes. The parent verified all seven recorded source hashes.

Controls scrolling, pinch zoom, exact hardware text entry, automatic keyboard
appearance, blur dismissal, floating-keyboard interaction, active-contact
cancellation, lifecycle stress, physical devices and FPS remain unverified.
The reset button does not blur the input. Hardware tool typing produced
Option-like glyphs; exact `ely` came from software-keyboard AX clicks.
Startup assertions are deterministic tests, separate from actual UI evidence.

Debug builds execute adapter regression assertions before opening the UI. These
cover origin conversion, complete snapshots, final position, cancellation,
second-finger preview cancellation, exclusive pinch, foreign windows, latched
routing, inset max, and narrow floating keyboards. ELY_ROUTE/ELY_MOBILE traces
identify actual OS events separately from ELY_ADAPTER_SELFCHECK assertions.

Android's exported `android_main` initializes JNI, then uses `jni::shared_platform`
and Ely `Assets` for the same root UI. The blocking Android event loop retains
the Application for its lifetime. No second platform is constructed.
Android runtime validation is unavailable: elevated `adb devices` found no
connected device, and the SDK emulator executable is absent. The keyboard
assertions executed on iOS as described above; Android runtime remains unverified.
