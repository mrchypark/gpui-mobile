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

- Ely: live local `ely-mobile-mrchypark/work/ely` checkout, branch
  `feature/mobile-ui`, HEAD `f98d0005e1805405cf8f71d758e57714a6bf9a25` plus
  owner-approved dirty changes. The previous published pin
  `7f6a3f91ceb6e6f6c2dbf4e43d49e01201e384cd` lacks the integration's required
  `Menu::usable_bounds`, `Menu::on_open_change`, and
  `TextInput::input_owner_observer` APIs.
- GPUI and gpui_wgpu: Zed `1a28cff4b409169bac058bca40dfbfeb7621d19b`.
- wgpu: crates.io `29.0.4`; no gpui-pre packages.

This local integration requires the external live Ely checkout in this sibling
layout; checking out HEAD alone does not reproduce its dirty changes:

```text
<parent>/gpui-mobile-mrchypark/work/gpui-mobile/examples/ely-integration/
<parent>/ely-mobile-mrchypark/work/ely/
```

The manifest explicitly uses `../../../../../ely-mobile-mrchypark/work/ely`
relative to this example. It uses no hidden Cargo override or immutable output
export. The current r5 dirty-source inventory is
`ely-mobile-mrchypark/outputs/resumed-validation-20261018/ely-live-r5.sha256`
relative to `<parent>`, with inventory SHA-256
`3e2e76ae70ff82879cdfc236993d3aff786b7bab2181db0534a7822a1e481b86`.
The r4 inventory, SHA-256
`fe296c2cb02bdb87b0d98260d8b46d303d499566c6019b41b65182b303c22de7`,
predates the Forms caller/test-only repair; library/API code is unchanged.
The r3 inventory, SHA-256
`6d32847bc22e376e55994857c2c462570cec1d7e228f1ac86732d3286a32453b`,
predates the final Tabs, Markdown, and Generative repairs.
The earlier r2 inventory, SHA-256
`56cd347ef941b273aec49a764a53791fe717918c3b7501e0752a9f0c58f21049`,
predates the reviewed menu and responsive repairs.
The earlier `ely-live-frozen.sha256` inventory, SHA-256
`597f975e0aa89bba82a44c6a9e04587277f36b3b3c89fe0ba0562d76ef571f5a`,
is historical: it predates the gallery `files/touch_tests.rs` lifetime fix.
Recorded Ely source SHA-256 values are:

| File | SHA-256 |
| --- | --- |
| `Cargo.toml` | `609589dbb8b26d7cd4e5e9cb7a9d8316fc70c3ee25bd211b0d939049819b3d25` |
| `src/menus/model.rs` | `64ef4b673c2ac9abc9011760c2c1832b58495fe6bcb62fc95d0b47f7343100fa` |
| `src/forms/text/owner.rs` | `37b669ca0bdd498285bb8f703dcc57e333461cc09bb43479adcb7507b9ea23eb` |

A published deployment pin containing these APIs is unresolved. This path is
for local integration only; a fresh standalone clone cannot build it without
the matching external checkout and dirty source. The lockfile refresh changed
only Ely's source. The locked dependency tree, macOS/iOS Simulator/Android target
metadata, and both existing host self-check tests passed from this project path.
An earlier unfiltered offline metadata attempt stopped on the missing Linux
cache entry `accesskit_atspi_common v0.19.1`; it is not a source compile failure.

The corrected local integration built, linked, and signed for iOS Simulator
(executable SHA-256
`38d9bd564e3d4fe1c5cb93d303f3a1f12c5bcfc010db825e927fc2e30be1dcd0`).
Installed-hash verification, both startup self-check markers, real menu selection
and outside dismissal without canvas commits, and B `나` → A → B `x` preserving
`나x` passed. A `a` remained separate. This is bounded iOS evidence, not complete
IME, gesture, Last-item scrolling, floating-keyboard, or lifecycle acceptance.

Android Rust compilation, packaging, signature, ABI, and merged-manifest checks
passed (APK SHA-256
`ded6b1e975a77a3c08a5a088c2bc4cfc7f0fb1df7338cc79531cd627d6e92ec3`).
The Gradle-only retry used installed JDK 21 after the inherited JDK 11 failed;
Rust was not rebuilt. On the existing API 36 emulator, the installed APK hash
matched, both startup self-checks passed, and native initialization plus the
resumed Activity were observed. The initial Activity launch wait timed out before
initialization completed. Codex Computer Use could not bind the emulator window,
so Android full UI/runtime acceptance: NOT VERIFIED (PENDING). Only narrow startup smoke
passed. Visual/menu/input acceptance remains unverified. The owned app and
emulator were stopped with absence verified. These results do not establish
physical GPS/BLE operation or permission grants. The merged manifest
does not declare Bluetooth scan/connect permissions; BLE is disabled in this
example's build. This documentation update follows the recorded builds and did
not change their code, manifest, or lockfile identity.

Historical `evidence/source-identity*.json` records actual Cargo metadata package IDs and
full sources for both iOS and Android. Exactly one of each package was found;
the identities match between targets. These records predate the local source
change and do not validate this local integration. The current verification
records are in the task's `outputs/existing-integration-original-20261018` and
`outputs/integration-original-native-20261018` directories; historical trees
remain in `evidence/*-ios.txt` and `evidence/gpui-android.txt`.

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
Android full UI/runtime acceptance: NOT VERIFIED (PENDING). APK-hash match, both startup self-checks, native initialization, and resumed Activity are narrow startup-smoke evidence only.
