# Location service integration

Use `gpui_mobile::packages::location` for asynchronous one-shot fixes and
foreground and explicitly requested background location streams, independently of Ely and the canvas.

## Setup

The `location` feature is enabled by default and exports `packages::location`.
If default features are disabled, enable `location` explicitly. Streams use the
existing `futures = "0.3"`; no additional Rust runtime dependency is needed.

On iOS, the library's `build.rs` compiles `src/packages/location/GpuiLocation.m`
with ARC through `cc` when `location` is enabled, and emits link directives for
Foundation, CoreLocation and UIKit. Consumers do not need a separate Xcode source
entry for the bridge. It calls the Rust `gpui_location_event` callback. Its C
symbols are `gpui_location_start`, `gpui_location_stop`, `gpui_location_enabled`,
`gpui_location_is_main_thread`, and `gpui_location_last`.

On Android package the updated `dev.gpui.mobile.GpuiLocation` and `GpuiLocationService` Java helpers (already
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
- `Ready { provider, background }` is emitted only after the native update
  registration call returns successfully. It does not mean a fix has arrived,
  GPS has satellite lock, or background continuity is verified. `Paused` clears
  registration feedback during foreground-watch suspension. The demo separates
  registered/waiting (with elapsed seconds), received position, paused, error and
  ended states; starting an Android foreground service alone never implies Ready.
- `get_current_position_async(&settings).await` waits for one fresh fix, ignores
  pre-request cached samples and has a native 30-second deadline. Dropping its
  future unregisters the listener. There is no UI polling/sleep/latch.
- `get_current_position_with_timeout(&settings, Duration).await` uses a caller
  deadline of 1 ms through 24 hours, in whole milliseconds. The deadline runs on
  the native main queue, including time spent waiting for permission or foreground
  resume; delivery requires that queue to be scheduled. It is not a real-time timer.
  Timeout emits an error and removes the listener; caller cancellation closes
  the Rust mailbox immediately and schedules native removal.
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
  foreground tracking stream; it can recover after permission is granted in Settings.
- `reconfigure(&new_settings)` changes accuracy/displacement with a new callback
  token and discards the old watch's queued samples. Invalid settings leave the
  existing watch untouched. Native configuration/start failures arrive on the new
  stream; this is not a transaction promising the old native watch survives failure.
  Cancelled/terminal subscriptions cannot be refreshed or reconfigured.
- Ordinary watches stop/resume on native background/foreground notifications.
  Android Activity destruction ends ordinary watches. Background watches use the
  separate explicit API and lifecycle described below.
- Horizontal accuracy is meters; timestamps are unsigned Unix milliseconds.
  Unknown optional altitude/speed/heading/accuracy measurements are `NaN`, not
  fabricated zero measurements. Approximate vs precise permission is an event;
  actual accuracy still comes from each sample. `Position` retains all fields.
- The callback mailbox holds at most 64 events. A slow consumer receives an
  explicit overflow error and an ended stream instead of unlimited growth.
  Drop/recreate the subscription after overflow. Native callbacks never wait
  for an async consumer. Native callback delivery uses short mailbox locks.
- iOS accuracy maps to the CoreLocation constants. Android prefers Google Play
  services FLP when available (`play-services-location:21.4.0` in both hosts).
  GMS requests a desired/minimum 1000 ms interval, zero batching delay and initial
  historical age, and does not wait for improved initial accuracy. Coarse-only
  permission uses balanced priority; fine permission maps High/Best/navigation
  to high, Medium to balanced, and Low/Lowest to low power. No cadence is guaranteed.
  All samples in a batch are sorted by monotonic fix time, then pass the shared
  freshness, validity and duplicate/out-of-order checks. Cache previews are separate;
  the existing cache API still reads platform provider caches, not GMS lastLocation.
- GMS availability, settings or registration failure falls back to API31+ platform
  fused, then permitted GPS/network. Security failures remain permission errors.
  Settings resolution is user-owned: the library never launches a resolution dialog;
  diagnostics report `settings_resolution_required` and try the platform path.
  Ready identifies `gms_fused` or `platform_<provider>` after registration succeeds.
  A fallback remains selected for that subscription; create a new watch to retry GMS.
  Pending registration cancellation fences callbacks and removes updates immediately
  and again after late completion. Fallback/restart waits for confirmed removal;
  removal failure emits/logs cleanup failure. Logical Stop releases its FGS lease
  and removes its stream immediately, but a separate retirement registry retains
  the native attempt until removal succeeds. It retries up to three times at one
  second spacing; a later start/refresh retries again, and remains blocked while
  cleanup is pending. No old-token refresh is required to retain cleanup ownership.
  Logical cancellation always rejects further delivery, but a failed native removal
  is not reported as successful cleanup. Task callbacks use a main-Looper Handler
  executor, including on API26/27.
- Android provider disablement first tries a remaining permitted provider. It
  emits a disabled error only when none is available, so a viable fallback does
  not prematurely fail a one-shot. Enabling a registered provider
  can resume it. If no provider was available at registration, call `refresh()`
  after enabling location (or foreground the activity).
- `distance_between` and `bearing_between` use spherical Earth geometry, meters
  and clockwise degrees from north. Invalid/nonfinite coordinates return NaN;
  antipodal distance is clamped against floating-point roundoff. Coincident
  bearing is zero by convention; poles/antipodes have no unique initial bearing.

## Permission and service convenience

With `permission_handler` enabled, `check_location_access()` returns independent
service and permission status without prompting. `request_location_access()`
delegates to the existing WhenInUse permission owner, on a worker thread. Neither
helper loops permission prompts. `LocationAccess::guidance()` distinguishes
disabled service, requestable denial, Settings-only denial and restriction.
Approximate versus precise access remains a native stream event; a grant or a
High request does not guarantee measured accuracy. Missing host usage strings or
capabilities are errors, not permission denials to retry automatically.

Always escalation is a separate explicit user action through
`permission_handler::request_permission(Permission::LocationAlways)`. No location
watch automatically asks for it. The GPS demo exposes this action separately.

## Background tracking

`watch_position_background(&settings)` opts in; keep its subscription alive and
drain events. Start a new background watch from the visible/active app after
permission is granted. Drop/cancel ends only that GPS watch; the final Android
GPS lease stops its location service. GPS and BLE have separate service classes,
notification IDs and ownership; stopping one must not stop the other.

Android requires coarse/fine permission and these manifest declarations:

```xml
<uses-permission android:name="android.permission.FOREGROUND_SERVICE" />
<uses-permission android:name="android.permission.FOREGROUND_SERVICE_LOCATION" />
<!-- Inside application -->
<service android:name="dev.gpui.mobile.GpuiLocationService"
    android:exported="false" android:foregroundServiceType="location" />
```

The foreground service starts from a resumed Activity and promotes before
requesting updates. It posts an ongoing notification with a GPS-only Stop action.
Android 13 notification permission affects notification-drawer visibility; the
service still has Android's foreground-service disclosure. Applications should
offer notification permission explicitly rather than silently request it here.
Application context owns background listeners, so Activity pause/destruction does
not itself stop them. Permission loss terminates background watches; grant again
and explicitly start a new watch. Service destruction ends its watches. A new
acquire while final-service teardown is pending fails with `background_start`;
retry after teardown from the visible app. It never reports a stopping service ready.

This implements foreground-initiated continuation, not a background-start bypass.
It does not automatically request ACCESS_BACKGROUND_LOCATION, launch at boot,
restart a killed process, or silently restore a stopped session. Android's
[location FGS requirements](https://developer.android.com/develop/background-work/services/fgs/service-types#location)
and [background-start restrictions](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start)
still apply. An OS/vendor shutdown cannot be reported by an already-dead process.

iOS requires `UIBackgroundModes` containing `location` and the WhenInUse usage
string. The explicit Always permission flow additionally requires
`NSLocationAlwaysAndWhenInUseUsageDescription`. Background watches enable
`allowsBackgroundLocationUpdates` and the visible background indicator, and do
not stop on app backgrounding. WhenInUse authorization can continue an already
started background-enabled session; Always is not fabricated as a prerequisite.
See [Apple authorization behavior](https://developer.apple.com/documentation/corelocation/requesting-authorization-to-use-location-services)
and [background updates](https://developer.apple.com/documentation/corelocation/cllocationmanager/allowsbackgroundlocationupdates).

New iOS background watches reject inactive/background starts. Refresh or
reconfigure may continue in background only when its exact predecessor token is
still active, background-enabled and updating; unrelated/stale/cancelled tokens
do not confer eligibility. Revocation ends background tracking. There is no
process-death restoration, significant-change relaunch, force-quit recovery or
guaranteed delivery cadence. These remain unsupported capabilities, not hardware
tests implicitly passed by background-mode configuration.

Canonical example and Ely Android manifests contain independent GPS/BLE service
entries. Ely Gradle directly uses canonical host Java; its iOS simulator staging
script copies canonical `example/ios/Info.plist`, preserving both `location` and
`bluetooth-central`. Rebuild/stage after changes; old installed apps are not updated
by a source edit.

## Standard-example GPS screen

`example/src/screens/gps.rs` uses the location-only `demo` worker. It offers
preflight, permission/Always actions, Settings, cache, fresh 30s/5s fixes,
foreground/background watches, Medium/10m reconfiguration, refresh and Stop.
Navigation dismisses repaint polling only: the GPS session persists until Stop,
so the BLE screen can be used simultaneously. Native lifecycle owns pause/resume;
the router must not cancel GPS on ordinary navigation or app pause. Stop cancels
pending async fixes/watches; an already-visible OS permission dialog cannot be
forcibly dismissed, and its worker finishes after that helper returns.
Delivery count, receipt Unix-ms time, fix time and accuracy are visible without
coordinates or coordinate logs. The router handoff is tracked separately; the
screen's existence alone is not route-integration or runtime evidence.

## Current evidence, 2026-10-16

The physical Android High fresh one-shot previously passed on R5KL705C28B:
17.743 seconds, reported accuracy 22.17908 m, timestamp present. Evidence:
`outputs/android-gps-diagnostics-retry1-20261016/gps-runtime.log` in the task
workspace. The earlier 30-second failure is preserved separately. This is a
historical end-to-end foreground fix, not proof of these newer background,
reconfiguration or timeout-option changes. No repeated device check is claimed.

Focused current checks and source identities are recorded under
`outputs/location-convenience-20261016`. Rust tests exercise actual mailbox,
timeout/cancellation/reconfiguration and demo ownership code with a host-only
native-start adapter. Java executes production service lifecycle state and fix
validation; SDK compilation is separate from Android runtime execution. iOS
delegate regression source covers visible-start eligibility, exact continuation,
background delivery and revocation; syntax checking is not execution. Real
screen-lock/background delivery, simultaneous GPS/BLE independence, permission
revocation and native listener cleanup remain explicit device acceptance gates.

## Evidence and limitations

Service-only tests and native compilation results are recorded below.
Passing a Rust type check or Objective-C/Java compilation does not demonstrate
app linkage, permission prompts, GPS quality, background behavior or device fixes.
Real devices must test approximate/precise grants, denial and Settings recovery,
GPS/provider loss, pause/resume, cancellation during a permission prompt and
concurrent subscriptions. Simulator-injected coordinates cannot establish radio
accuracy or energy consumption.

Primary API references: [CoreLocation updates](https://developer.apple.com/documentation/corelocation/cllocationmanager/startupdatinglocation()),
[Android LocationManager](https://developer.android.com/reference/android/location/LocationManager).

For the separate opt-in BLE API and its platform setup, see
[BLE integration](ble-integration.md).

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
# Use an already-resolved Android host classpath: one colon-separated line of
# absolute jar paths including android.jar, GMS location/base/basement/tasks
# and transitive dependencies. No test performs Gradle resolution/downloads.
export LOCATION_JAVA_CLASSPATH_FILE=/absolute/path/to/javac-classpath.txt
# Optional JAVA_HOME selects the JDK; otherwise javac/java must be on PATH.
# Android SDK compilation, filter/service tests, and deterministic GMS Task tests:
sh src/packages/location/tests/check.sh --android-only

# Omit LOCATION_SIMULATOR_UDID to compile without launching a simulator process.
# Full runner also executes offline Rust and native compilation (coordinate slot).
LOCATION_SIMULATOR_UDID=<booted-simulator-uuid> \
  sh src/packages/location/tests/check.sh
```

A native owner now sends `Ended { reason }` and closes the mailbox before removing
its listener/timeout. Android activity destruction and iOS configuration failures
therefore cannot leave a one-shot or tracking stream waiting forever. Normal
permission denial in tracking remains recoverable. Explicit Rust `cancel()` ends
the stream immediately and clears queued events; it does not emit `Ended`.

The final existing service harness rerun passed all nine
Rust tests after malformed-event hardening. The new `tests/check.sh` runner passed
shell syntax validation, but its full clean run and cached retry both hit
`ENOSPC` during Cargo output (128 MiB and falling on the shared volume). Its
end-to-end invocation is therefore **not yet verified**. The
previous successful target checks and simulator native regression run remain
separate evidence. Existing simulator artifacts and test diagnostics were kept.
