# Native Ely mobile viewport

A small UIKit app using actual Ely Button/Input/InfiniteCanvas components,
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
to count as bottom occlusion. Floating rectangles remain in WindowInsets but
floating-keyboard focus avoidance is unsupported. Android IME bottom insets also
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

The Mac was locked during attempted UI verification. **No actual clicks, drags,
pinches, text entry or scroll gestures were performed.** Those interactive gates,
keyboard focus behavior, lifecycle stress/recovery, physical devices and FPS
remain unverified. Startup assertions are deterministic adapter tests, not
hardware gesture evidence. No UI lock bypass was attempted.

Debug builds execute adapter regression assertions before opening the UI. These
cover origin conversion, complete snapshots, final position, cancellation,
second-finger preview cancellation, exclusive pinch, foreign windows, latched
routing, inset max, and narrow floating keyboards. ELY_ROUTE/ELY_MOBILE traces
identify actual OS events separately from ELY_ADAPTER_SELFCHECK assertions.

Android's app path uses jni::shared_platform after init_platform, rather than
constructing a second platform. Android metadata identity passed; this example
has no APK host and no Android runtime validation.
