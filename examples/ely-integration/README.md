# Native Ely mobile viewport

The integration uses GPUI Kit 0.8.1, GPUI Fast 0.1.12, and a pinned public
Ely fork revision. Cargo applies the local Fast overrides in the application
workspace; dependencies do not inherit patches from other manifests.

Earlier validation records below describe previous dependency snapshots and do
not validate the current one.

## Android Activity recreation regression

Build with `GPUI_LIFECYCLE_STRESS=1` to add a delayed timer and an external
thread wake to each Activity. Install the debug app and its Android test APK,
then run the `recreate_stress` instrumentation scenario. The test recreates
the Activity three times in the same process, waits for the old tasks' wakeups,
and verifies that the last native window is ready. The separate `background`
scenario checks foreground restoration without recreation.

```sh
GPUI_LIFECYCLE_STRESS=1 ./examples/ely-integration/android-apk.sh
./example/android/gradle/gradlew -p examples/ely-integration/android :app:assembleDebugAndroidTest
adb install -r examples/ely-integration/android-build/ely-integration-arm64-debug.apk
adb install -r examples/ely-integration/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk
adb shell am instrument -w -e scenario recreate_stress dev.gpui.mobile.ely.test/dev.gpui.mobile.example.ActivityLifecycleTest
```

The app's `android_main` must retain its `init_platform` return value until
`Application::run` returns. The session cancels Activity-local foreground tasks
before releasing the native thread. Do not bypass the GPUI foreground executor
with its hidden raw scheduler for tasks that can outlive an Activity.

## Opt-in platform smoke checks

Set `GPUI_PLATFORM_SMOKE=1` when running `android-apk.sh` or
`ios-simulator.sh` to check package and device information, battery availability,
the camera permission status, cache directory, and an isolated preferences
write/read/remove. The app writes `gpui-platform-smoke.txt` in its cache directory.
The connectivity API is invoked but its result is not independently validated.
The checks do not request permissions or read user content; a battery value of
`-1` is reported as skipped. These checks do not cover camera capture, sensors,
media, sharing, notifications, or network reachability.

Set `GPUI_PLATFORM_EXTENDED=1` for additional sensor samples, location-service
and camera discovery, and silent audio and video playback using generated or
bundled test media. Camera session creation and a current location fix run only
when the test app already has the corresponding permissions; coordinates are
not written to the results. Results are in `gpui-platform-extended.txt` in the
app cache. Unavailable hardware is reported as skipped. This does not test
camera capture, notification delivery, Bluetooth, or permission prompts.

On Android, `GPUI_PLATFORM_DEVICE=1` instead requests camera and notification
permissions, attempts a low-resolution photo, removes the resulting test image,
and posts a temporary notification for 20 seconds before canceling it. Inspect
`gpui-platform-device.txt` in the app cache and independently check the OS
notification list. Permission results report the final status; only a fresh
permission prompt tests the request UI. The camera and notification checks can
fail if the activity is not in the foreground.

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

BLE is enabled by default in this Android build, including its matching Java
backend and foreground-service declaration. Set `GPUI_MOBILE_BLE=0` to omit it.
Android permissions are requested by the application when needed, not granted
by the manifest declarations.

The distinct output is `android-build/ely-integration-arm64-debug.apk` beneath
this example, with application ID `dev.gpui.mobile.ely`, label `Ely Integration`,
and native library `gpui_mobile_ely_integration`. This is the Ely UI, not the
BLE demo. The manifest declares network and BLE permissions; runtime permissions
still require the user's grant.

The final arm64 cdylib and debug APK built successfully on 2026-10-15. Manifest,
NativeActivity/JNI exports and APK v2 debug signature verification passed; see
`evidence/native-android.txt` for hashes and commands. AndroidX adds biometric,
fingerprint and internal receiver permissions through manifest merging.

## Historical source identity (2026-10-15)

- Ely: published `https://github.com/mrchypark/Ely-GPUI-Components`,
  `7f6a3f91ceb6e6f6c2dbf4e43d49e01201e384cd`.
- GPUI and gpui_wgpu: Zed `1a28cff4b409169bac058bca40dfbfeb7621d19b`.
- wgpu: crates.io `29.0.4`; no gpui-pre packages.

`evidence/source-identity*.json` records actual Cargo metadata package IDs and
full sources for both iOS and Android. Exactly one of each package was found;
the identities match between targets. `evidence/*-ios.txt` and
`evidence/gpui-android.txt` are historical dependency trees, not the current lockfile.
Local checkout paths in the restored evidence are redacted.

This is an independent workspace/lockfile. The app root repeats the existing
async-task patch because dependency manifest patches are ignored. The lock
uses libc 0.2.191 for the Android arm64 ABI fix and iOS `_dyld_*` declarations.
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
focused input is revealed in the laid-out scroll viewport when focus, keyboard
geometry or viewport bounds change while the keyboard is visible. This also
covers Android resizing the viewport without reporting a keyboard rectangle.
For a floating keyboard, the controls use the closest clear strip above or below
it. If neither strip in the controls is tall enough, the controls can borrow
space from the canvas. If neither strip in the full window fits, scrolling
cannot fully reveal the input. Unchanged paints and hidden-keyboard updates
preserve manual scrolling. Android IME bottom insets participate in the max.

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

On 2026-10-16, 25 CUA individual BackSpace presses cleared the field, then separate
`pressKey` calls for a, b, c, 1, 2, 3 produced exact `abc123` on the same executable
(local `a08990b`, remote baseline `7253295`). See `09-hardware-abc123.png` in the
UI evidence outputs. Individual tool-driven hardware-key input passed; human
physical-keyboard typing was not tested. Bulk `typeText('abc123')` still produced
`å∫ç123`; that tool/Simulator-path discrepancy remains unresolved and does not
justify a backend change.

Controls scrolling, pinch zoom, automatic keyboard
appearance, blur dismissal, floating-keyboard interaction, active-contact
cancellation, lifecycle stress, physical devices and FPS remain unverified.
The reset button does not blur the input. Exact `ely` came from software-keyboard
AX clicks in the earlier dated verification.
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

The optional `ely-owner-reporter` feature registers Ely's app-wide input-owner
reporter for every text input. The pinned Ely revision includes
`forms::set_input_owner_reporter`. Enable this feature for compound controls as
well as individually configured inputs.
