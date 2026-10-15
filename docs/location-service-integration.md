# Location service integration

Ownership: GPS service files only. GPUI/wgpu pins, root manifests, central exports,
renderer, and platform lifecycle files are unchanged by this work.

## Required parent integration

`location` already exists as a feature and `packages::location` is already exported.
No Rust dependency is added: streams use the existing `futures = "0.3"`.

On iOS compile `src/packages/location/GpuiLocation.m` with ARC for the exact app
architecture and SDK, and include the resulting object in the final app link.
Link Foundation, CoreLocation and UIKit. The Objective-C bridge references the
Rust `gpui_location_event` callback, so the app must enable the `location` feature.
A build script using `cc` or an explicit Xcode source entry can perform this step;
the manifest/build owner must choose the integration. The required exported C
symbols are `gpui_location_start`, `gpui_location_stop`, `gpui_location_enabled`,
`gpui_location_is_main_thread`, and `gpui_location_last`.

On Android package the updated `dev.gpui.mobile.GpuiLocation` Java helper (already
included by the example Gradle source set). The JNI native callback is exported
from `src/packages/location/android.rs`; no central JNI module edit is needed.

Apps must provide iOS `NSLocationWhenInUseUsageDescription` and Android coarse/fine
location manifest declarations. Android runtime permission UI belongs to the app;
request coarse and fine together when precise access is needed. iOS requests
When In Use on the main queue. Do not use the existing blocking permission helper
on a UI thread. GPS does not depend on Ely, the canvas or GPUI event handling.

## API and behavior

- `watch_position(&settings)` returns `LocationSubscription`, a `Stream` of
  permission changes, positions, and native errors. Start it while foregrounded.
- `get_current_position_async(&settings).await` waits for one fresh fix, ignores
  pre-request cached samples and has a native 30-second deadline. Dropping its
  future unregisters the listener. There is no UI polling/sleep/latch.
- Existing synchronous `get_current_position` callers still compile. It waits
  only on worker threads and rejects UIKit main / Android looper threads. The
  example's existing background-executor caller remains compatible.
- `get_last_known_position` never requests a fix. Android reads provider caches;
  iOS returns the latest sample observed by this service in this process, which
  may be stale or absent. Check its timestamp before use.
- Drop or `cancel()` invalidates the callback token immediately, then schedules
  native listener removal. Already in-flight callbacks are discarded.
- `refresh()` restarts an active tracking subscription and rechecks permissions.
  Returning to foreground also rechecks permission. Denial does not end a
  tracking stream; it can recover after permission is granted in Settings.
- Native foreground/background notifications stop/resume tracking. No iOS Always
  authorization, background location mode, Android foreground service or ongoing
  background tracking is implemented. Android activity destruction cancels its
  listeners; a recreated activity must create new subscriptions.
- Horizontal accuracy is meters; timestamps are unsigned Unix milliseconds.
  Unknown optional altitude/speed/heading/accuracy measurements are `NaN`, not
  fabricated zero measurements. Approximate vs precise permission is an event;
  actual accuracy still comes from each sample. `Position` retains all fields.
- The callback mailbox holds at most 64 events. A slow consumer receives an
  explicit overflow error and an ended stream instead of unlimited growth.
  Drop/recreate the subscription after overflow. Native callbacks never wait
  for an async consumer. Native callback delivery uses short mailbox locks.
- iOS accuracy maps to the CoreLocation constants. Android selects GPS or
  network based on accuracy and permission, requests at most 1 Hz nominally,
  and passes through the minimum displacement. OS delivery may be less frequent.
- Android provider disablement emits an error; enabling a registered provider
  can resume it. If no provider was available at registration, call `refresh()`
  after enabling location (or foreground the activity).

## Evidence and limitations

Service-only tests and native compilation results are recorded after validation.
Passing a Rust type check or Objective-C/Java compilation does not demonstrate
app linkage, permission prompts, GPS quality, background behavior or device fixes.
Real devices must test approximate/precise grants, denial and Settings recovery,
GPS/provider loss, pause/resume, cancellation during a permission prompt and
concurrent subscriptions. Simulator-injected coordinates cannot establish radio
accuracy or energy consumption.

Primary API references: [CoreLocation updates](https://developer.apple.com/documentation/corelocation/cllocationmanager/startupdatinglocation()),
[Android LocationManager](https://developer.android.com/reference/android/location/LocationManager).

BLE is assigned separately: no BLE implementation or dependency change is part
of this work. Library reconnaissance found [Bluest](https://github.com/alexmoon/bluest)
supports iOS but describes Android support as planned; this is not a dependency
selection or a claim of complete cross-platform BLE support.

## Verification, 2026-10-15

- Host Rust tests: **9 passed**, including token cancellation, bounded overflow,
  permission-denied/granted delivery, malformed positions, optional metadata,
  native-owner destruction completing a one-shot immediately, and a terminal
  configuration error ending a tracking stream. No timeout is needed to make
  the owner-destruction test pass.
- Rust 1.97.1 service-only `cargo check`: **passed** for
  `aarch64-apple-ios-sim` and `aarch64-linux-android`. The Android harness used
  the actual repository JNI helper bodies, with external app pointer accessor
  declarations; it checks JNI types/signatures but is not a linked Android app.
- Objective-C ARC build with `-Wall -Wextra -Werror`: **passed** using the iOS
  Simulator 26.2 SDK and an iOS 16 deployment target.
- Android helper `javac` against Android API 34: **passed**. The compatibility
  `LocationListener.onStatusChanged` override is deprecated but retained for
  older Android versions. No Android device execution was performed.
- The native delegate regression executable **linked and ran successfully** on
  booted iPhone 17 Pro / iOS 26.3 Simulator. It uses synthetic locations and a
  test manager, exercising permission recovery, pause/resume, cancel, a late old
  callback after replacement, stale vs fresh one-shot fixes and terminal missing
  usage-description errors. It does not request GPS or show a permission prompt.
- Running that executable from Documents stalled in the simulator's dynamic
  loader before `main`; the identical executable from `/private/tmp` passed.
  The scoped test runner therefore builds its test executable under `/private/tmp`.

Reproduce host/native compilation and, optionally, simulator callback execution:

```sh
# Omit LOCATION_SIMULATOR_UDID to compile without launching a simulator process.
LOCATION_SIMULATOR_UDID=<booted-simulator-uuid> \
  sh src/packages/location/tests/check.sh
```

A native owner now sends `Ended { reason }` and closes the mailbox before removing
its listener/timeout. Android activity destruction and iOS configuration failures
therefore cannot leave a one-shot or tracking stream waiting forever. Normal
permission denial in tracking remains recoverable. Explicit Rust `cancel()` ends
the stream immediately and clears queued events; it does not emit `Ended`.

Disk coordination note: the final existing service harness rerun passed all nine
Rust tests after malformed-event hardening. The new `tests/check.sh` runner passed
shell syntax validation, but its full clean run and cached retry both hit
`ENOSPC` during Cargo output (128 MiB and falling on the shared volume). Its
end-to-end invocation is therefore **not yet verified**. No further Cargo work is
running from the GPS worker; the parent owns sequencing subsequent builds. The
previous successful target checks and simulator native regression run remain
separate evidence. Existing simulator artifacts and test diagnostics were kept.
