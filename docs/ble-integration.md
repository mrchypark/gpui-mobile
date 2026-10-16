# BLE integration

Enable the opt-in `ble` feature to use `gpui_mobile::packages::ble`. The feature,
module export, and optional dependencies are already configured in the library:


```toml
[features]
ble = ["dep:btleplug", "dep:tokio"]

[dependencies]
btleplug = { version = "=0.13.5", optional = true }
tokio = { version = "1.53.1", features = ["rt", "time"], optional = true }
```

Existing `futures = "0.3"` is reused. No UUID dependency addition is needed.
Applications must run BLE futures on a Tokio runtime with time
and I/O enabled (desktop Linux needs I/O). Do not `block_on` the UI thread.
Android requires its existing jni 0.22 dependency and a Java-side invocation of
`ble::init_android(&mut jni::Env)` before `Ble::new`. Include the **matching**
btleplug 0.13.5 Java library (API >=24) and JNI utility classes in the APK;
these are not supplied by Rust linking. Details and verification follow below.

Android wiring is implemented in the example:

- `GpuiActivity` invokes `Ble.initialize()` after loading the native library and
  before `NativeActivity.onCreate`, only when `BuildConfig.BLE_ENABLED` is true.
- From `example/android/gradle`, `./gradlew -Pble=true :app:assembleDebug`
  prepares checksum-verified Java sources
  from the exact 0.13.5 crate and runs cargo-ndk with `--features ble` before APK
  packaging. The example forwards `ble = ["gpui-mobile/ble"]`.
- Default Gradle builds neither initialize BLE nor require BLE Rust symbols.
- Required Android declarations and JNI keep rules are included. The app must
  request SCAN/CONNECT at runtime on API 31+, or fine location on API <=30.
  Location permission setup is described in [Location service integration](location-service-integration.md).
- `src/packages/ble/java` is compiled in both modes; the upstream Java sources
  are included only in BLE mode. The pinned patched backend is checked in under
  `src/packages/ble/vendor`; preparation must preserve its matching Java ABI.

## Backend selection and boundaries

Verified 2026-10-15 against published btleplug **0.13.5**, source commit
`1af8299c291f24fca68dfb6b7ef1b5c0475892e1`, crates.io checksum
`3d836502de731602ea87cbb307065e8feaed4fd10f0855a75b0b87944ee011dc`.
The crate provides CoreBluetooth and Android BluetoothGatt backends rather than
requiring a second handwritten GATT stack. Bluest was considered but its current
published documentation says Android is planned. The chosen backend supports
central/client operations, not advertising, peripheral/server mode, or Classic.
Neither GPUI nor Ely participates in these device services.

Primary sources:
- https://github.com/deviceplug/btleplug
- https://docs.rs/btleplug/0.13.5/btleplug/api/trait.Peripheral.html
- https://docs.rs/bluest/latest/bluest/
- https://developer.android.com/develop/connectivity/bluetooth/bt-permissions
- https://developer.apple.com/documentation/corebluetooth

## API and asynchronous flow

`Ble` owns one active peripheral and exposes `start_scan`, `stop_scan`, `connect`,
`discover`, `read`, `write`, `notifications`, `subscribe`, `unsubscribe`,
`disconnect`, and `reconnect`. Calls return native errors or timeout, never an
unsupported-operation success placeholder. Characteristic membership and flags
are checked before GATT use. A write without response means queued, not remote
acknowledgment. Scan start means the OS accepted the request, not that any device
was found. Obtain `events()` before scanning/connecting and `notifications()`
before subscribing. Streams wake the executor on callbacks; there is no polling
loop, sleep, UI-thread latch, or UI-thread block_on in the module.

Consume events continuously and await `observe(&event)` on the same session.
Disconnect events are reconciled with the current native connection state, so a
queued event for the preceding connection does not invalidate a new connection.
A failed state query conservatively invalidates GATT and returns the error.
`reconnect(&id)` performs one explicit attempt: disconnect, reacquire the native
peripheral (CoreBluetooth retrieve by opaque identifier; Android add by native
identifier), connect, rediscover. It uses the newly returned object directly;
merely looking up the removed CoreBluetooth cache or reusing its old Rust handle
would fail. Obtain a fresh notification stream and explicitly subscribe again.
The caller retains the ID, so retrieval timeout/cancellation can be retried even
when disconnect already cleared the session handle. There is no automatic retry
loop or implicit replay of application writes.

`Permission` is the host's observed OS permission, not a request API or a promise
of availability. Unknown/Denied/Restricted prohibit new operations. Feed native
permission callbacks to `permission_changed().await`; revocation gates first and
attempts both scan-stop and disconnect. On app pause await `pause`; on resume
refresh native permission and call `resume`, then explicitly scan/connect again.
Successful pause/disconnect releases the session's device handle: save the ID or
rediscover it in the application. Always inspect the cleanup error list. Cleanup
is still callable while paused, denied, or recovering. Await `close` before drop.
Dropping a Rust future cannot guarantee cancellation of a native OS request;
interrupted GATT marks the session as needing disconnect/reconnect before reuse.
If cleanup cannot complete, do not claim the radio stopped; retry after the OS
permission/radio recovers. Native operation timeouts bound waiting, not OS work.

## Platform restrictions

- iOS/macOS: bundle `NSBluetoothAlwaysUsageDescription`; native authorization is
  enforced by CoreBluetooth. Device identifiers are opaque UUIDs, not MACs.
  This foreground session intentionally disconnects on pause. State restoration,
  background wakeups, peripheral role, and guaranteed background streaming are
  not implemented. Do not add bluetooth-central background mode and infer that
  this creates an end-to-end background service.
- Android: API 31+ requires runtime SCAN/CONNECT; older scanning requires fine
  location and may require location services. The host must request permissions
  asynchronously via Activity callbacks and refresh on return from Settings.
  `neverForLocation` is deliberately not asserted for the map app: it may filter
  beacon results and is inappropriate without a product-level guarantee.
  No BLE foreground service/background scan/restart-on-boot is implemented.
- The local Android bridge forwards `onScanFailed` through `scan_errors()`.
  `scan_for` consumes it alongside events and returns failure, not empty success.
  Some native permission exceptions are `RuntimeError`, not `PermissionDenied`;
  do not determine Android permission state solely by matching the Rust error.
- Adapter event/notification channels in the dependency are bounded and can drop
  events for slow consumers. They are not a lossless telemetry log. Applications
  needing loss detection must use a peripheral protocol sequence counter.
- One session per adapter is the supported example policy: native scanning is
  adapter-wide. Multi-session scan arbitration is not implemented.

## Verification (2026-10-15)

The service checks below used a separate `work/ble-probe` manifest pointing at this module,
so no GPUI revision or wgpu dependency was altered to make BLE compile.

Completed before the disk-space stop:

- macOS native compile/link and **5 passed unit tests**, covering denied/recovered
  permission, pause/recovery gating, stale GATT capabilities, delayed disconnect
  reconciliation, async timeout, and propagation of a native-denial result.
- `cargo +1.97.1 check --target aarch64-apple-ios-sim`: passed.
- `cargo +1.97.1 check --target aarch64-linux-android`: passed, including JNI.
- `javac` against Android API 35: passed for all matching upstream Java sources
  plus our `Ble.java` JNI entry point (before Activity integration).
- `prepare-android.sh`: syntax check and real pinned download/checksum succeeded.
- Earlier Android link/SDK-download attempts exhausted disk. After freeing space,
  the final integrated BLE-enabled cdylib compiled and linked, and the complete
  debug APK assembled successfully. Final evidence and commands follow below.

After those checks, `tests/hardware.rs` was added and not compiled or run due to
the explicit disk-space stop. It is ignored by default and requires a consenting
read/write/notify echo peripheral and `BLE_ECHO_SERVICE`,
`BLE_ECHO_CHARACTERISTIC`, `BLE_ECHO_TEXT`. It exercises real scan, GATT exchange,
reconnect after cache removal, and a stale disconnect event against a live new
connection. Never run against an arbitrary device: it writes the requested text.
No physical BLE transaction, permission dialog/Settings recovery, radio loss,
pause/resume on a device, or notification delivery has been verified here.
The iOS simulator type check does not prove physical BLE availability.

## Host executor usage

The tested runtime pattern is `tokio::runtime::Builder::new_current_thread()`
with `enable_time()`. Keep its lifetime outside the BLE futures and run its event
loop on a dedicated application worker thread. The GPUI thread sends commands
and receives results through asynchronous channels. For example (app-owned
imports use `gpui_mobile::packages::ble::{Ble, Permission, Error, Result}`):

```rust
// Create this worker from an application service, never block_on in a UI callback.
let worker = std::thread::spawn(move || -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time().build()
        .map_err(|e| Error::RuntimeError(e.to_string()))?;
    runtime.block_on(async {
        let mut ble = Ble::new(Permission::Unknown, std::time::Duration::from_secs(15)).await?;
        // Await native permission/lifecycle commands, update ble, then await its
        // scan/GATT operations and continuously drain events/notifications here.
        // Unknown remains blocked until the host reports a genuine OS grant.
        let errors = ble.close().await;
        match errors.into_iter().next() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    })
});
// Do not join `worker` from the UI thread while BLE work is active.
```

This snippet is illustrative host integration. Runtime construction and async
operation signatures compile in the unit/probe checks; no production command
worker or permission dialog is created by this snippet. The hardware test is a
complete executable GATT sequence once a dedicated echo peripheral is supplied.
For desktop Linux use a runtime with I/O enabled as required by the BlueZ client;
the iOS/Android dependency feature set remains `rt,time`.

Final small re-run after disk cleanup: **6 passed, 0 failed, 1 ignored**.
The ignored physical echo test now compiles, including explicit-ID reconnect
when no active session handle remains. The sixth passing test is a compile-time
API contract proving cancellation does not consume the caller-owned retry ID;
it does not simulate an OS request in flight. The other five tests execute the
state/error logic. The final result log is retained at
`../ble-probe/ble-final-tests.log` relative to this repository. Its dedicated
109 MiB `target-final` directory was removed after successful completion.
The hardware characteristic environment value must be a hyphenated UUID.

## Final integrated Android package verification

On 2026-10-15, the latest FIFO/RawOnly/text and Android cleanup source compiled and
linked with BLE as an ARM64 cdylib in 26.07s. The final Gradle build succeeded in
1m02s (36 tasks), including checksum-verified `prepareBleJava`, Activity/BLE Java
compilation, and APK packaging. Build-Tools 36 used existing accepted SDK licenses;
no new license prompt was accepted.

After copying the independently verified BLE-enabled `.so` into
`example/android/gradle/app/src/main/jniLibs/arm64-v8a/`, run from
`example/android/gradle` with JDK 21 and the installed Android SDK environment:

```sh
sh ./gradlew --no-daemon --max-workers=1 -Pble=true :app:assembleDebug -x :app:buildBleRust
```

Only the redundant Rust task is excluded. `prepareBleJava` still invokes
`src/packages/ble/prepare-android.sh` and verifies the exact Java archive checksum.
The APK's native library SHA-256 matches the independently linked `.so`:
`66e37375997042539427fef79516596bcea8888f8659637dc15a8c62d6f5f565`.
APK signature verification passed. The preserved APK is
`outputs/gpui-mobile-example-ble-arm64-debug.apk`, SHA-256
`f9c08f42579fe64352acdfdc157b0f4db75158aafe064b7b571c99ba1aa4da72`.
See [compatibility](compatibility.md) for full build commands and evidence paths.
`adb devices -l` returned no connected targets. No APK installation, Android
execution, or physical BLE transaction is claimed.

## Current convenience and background contract (2026-10-16)

This section supersedes the historical foreground-only/background-not-implemented
statements above. Existing API signatures remain available; no dependency pins changed.

- `scan_for(ScanOptions)` subscribes before starting, scans for a finite 1ns–300s
  window, and attempts stop on both success and error. The operation has separate
  native start/stop deadlines, so total wall time can exceed the collection window.
  Results are deduplicated by native ID and refreshed on advertisements/RSSI changes.
  Service filters are ORed; case-sensitive name-prefix and minimum-RSSI filters
  are ANDed. Missing required metadata does not match. At most `max_devices`
  entries (1–4096, default256) are retained. A normal zero-result scan is not peer
  evidence. Native scan errors, stream closure and radio loss are errors.
- `connect_and_discover` attempts disconnect on preparation failure and reports
  cleanup failure alongside the original error. Cancellation still requires
  explicit `close`; native work is not canceled merely by dropping a future.
- `characteristic_by_uuid` requires canonical full service AND characteristic UUID
  strings. `subscribe_values` opens the stream before CCCD subscription and filters
  both UUIDs. Drop of the stream is not unsubscribe; call `unsubscribe` or `close`.
  Existing native property/capability validation and service addressing remain.
- `Ble::Drop` schedules bounded stop/disconnect on its creating Tokio runtime,
  without blocking UI or creating a second runtime. This is best-effort only:
  runtime shutdown can cancel the task. Cleanup failures emit fixed diagnostic
  text without peer information. Await `close` for observable native cleanup.

### Explicit background connection opt-in

While visible, obtain real permission, connect/discover the intended peer, obtain
notification streams and subscribe as needed, then await
`enable_background_connected()`. Call `pause()` when entering background and
`resume(actual_permission)` on foreground return. With valid native context and
no interrupted operation, pause stops scanning but preserves the connection.
Without opt-in, pause stops scanning and disconnects as before. Interrupted work,
permission denial, or lost context requires cleanup. `close()` always stops and
disconnects, including opted-in sessions. Call `permission_changed` for revocation;
`resume` alone does not asynchronously clean up a denied connection.

Android requires a visible Activity at opt-in, existing BLUETOOTH_CONNECT grant
on API31+, declared `FOREGROUND_SERVICE` and (API34+) matching
`FOREGROUND_SERVICE_CONNECTED_DEVICE`, and this service under application:

```xml
<service android:name="org.gpui.mobile.ble.BleConnectionService"
    android:exported="false" android:foregroundServiceType="connectedDevice" />
```

The service validates configuration before launch, posts an ongoing notification,
reports actual foreground-start success/failure, and is independently owned from
GPS. Startup waits at most3seconds; cancellation invalidates the lease so a late
service start cannot revive it. `START_NOT_STICKY` deliberately avoids creating
an empty session after process death. Android background-start restrictions still
apply; no silent grant, settings change, or restart loop is performed. Activity
recreation does not itself own the service; the application must retain its BLE
session and forward lifecycle events. Explicit session close ends the service.
The service does not grant indefinite CPU execution or immunity to OS/user kills.
See [Android foreground-service types](https://developer.android.com/develop/background-work/services/fgs/service-types).

On iOS, opt-in checks the running bundle for `UIBackgroundModes` containing
`bluetooth-central`. Add this alongside (not instead of) existing modes and retain
`NSBluetoothAlwaysUsageDescription`. CoreBluetooth may wake the process for
connection/notification events; this is not a guarantee that timers or arbitrary
Rust work run continuously. Scanning remains foreground-only in this API. The
backend does **not** implement restoration identifiers/delegate state restoration,
relaunch recovery, or force-quit recovery. Preserve the runtime/session while
alive and explicitly reconnect after process restart. See
[Apple background and restoration semantics](https://developer.apple.com/library/archive/documentation/NetworkingInternetWeb/Conceptual/CoreBluetooth_concepts/CoreBluetoothBackgroundProcessingForIOSApps/PerformingTasksWhileYourAppIsInTheBackground.html).

Background source support is distinct from runtime validation: locked-device
notification delivery, revocation, service loss and foreground return require
separate known-peer Android/iOS execution. None is established by the earlier
Android ten-second zero-discovery scan.

### Dedicated example and safe QCY inspection

`example/src/screens/ble.rs` is a dedicated standard-example screen. Parent wiring:
feature-gate its module with `ble`, route its `render(router,cx)` explicitly, call
`on_pause`/`on_resume` on host transitions and `dismiss` on navigation away.
No shared dispatcher or Ely integration files are modified by this module.
`ble::demo::Demo` uses one worker/runtime, bounded command submission, no native
operations on UI, explicit cancellation, and awaited cleanup before worker exit.
A busy operation is conservatively cancelled/closed on host pause; enable
background while idle after connection. Merely opening the worker does not scan,
connect, subscribe or write. The screen has explicit scan, select/connect/discover,
read, subscribe/unsubscribe, background opt-in and close controls. Peer metadata
and selected values appear locally, never in application logs. No write button
is exposed in the peer-inspection screen; the typed worker/API write command is
for applications with an explicit protocol/payload contract.

`qcy::read_known_values` uses community OpenQCY-Desktop revision
`ee7e849f2823e164dcf73726bc6ad700a9ce3281` as a **candidate** mapping: service
`0000a001-0000-1000-8000-00805f9b34fb`, firmware characteristic `00000007-…`,
battery `00000008-…`. It requires discovered READ flags, performs only those
service-qualified direct reads, and never sends command queries, initialization,
settings writes or subscription side effects. Missing capabilities return `None`;
native errors propagate. See the community
[UUID definitions](https://github.com/GiorgioRafael/OpenQCY-Desktop/blob/ee7e849f2823e164dcf73726bc6ad700a9ce3281/Protocol/QcyUuids.cs).
Select the user's exact QCY peer first; matching name/manufacturer is not device
identity or proof of firmware compatibility. The ignored echo hardware test now
requires `BLE_ECHO_ID` as well as the service, characteristic and authorized payload;
it must not be run against earbuds without an agreed echo protocol. QCY reads
cannot certify GATT writes, notifications or background delivery.

### Review follow-up: navigation and lifecycle wiring

`dismiss()` means **leave the BLE screen**, not explicit close. It stops UI polling
and cancels ordinary foreground sessions, but retains explicitly opted-in
background connections. `close()` is the unconditional explicit stop and is also
appropriate for final application-owner teardown. Do not call it merely because
the user navigates to GPS: simultaneous GPS and BLE need separate ownership.
Call `on_pause()` and `on_resume()` for the BLE session regardless of the visible
screen. Repeated pause/resume notifications are deduplicated. A permission dialog
pausing the initial open does not cancel that permission request. Resume stores
the real permission result for processing after an in-flight pause rather
than losing it to the normal busy-command gate.

Native notifications update total/background counts and last receipt UTC
milliseconds in the screen snapshot. `BLE_DEMO notification count=… background=…
background_count=… utc_ms=… elapsed_ms=…` diagnostics contain no address, device
name or value bytes. Only receipts processed after the worker handles pause count
as background. Capture these markers together with OS lifecycle timestamps for
acceptance; setting background mode alone is not notification-delivery evidence.

The optional scan-error side-channel is pending on iOS/non-Android. Regression
coverage verifies it neither aborts an idle scan nor blocks a live primary event,
while primary EOF and actual Android failure codes remain errors. Pause now
preserves a failed native background-context check even if fallback cleanup
succeeds; independent cleanup errors are returned alongside it.

The demo worker serializes pause and resume in a bounded eight-transition FIFO.
A quick foreground return cannot overtake a pending pause, and permission denial
is processed even if permission is subsequently granted. Overflow requests full
session cleanup rather than silently dropping a transition. Pause during ordinary
BLE work cancels that work and closes the session; pause/resume cleanup itself is
allowed to finish in order. These are worker-ordering guarantees, not evidence of
hardware background delivery.

The standard BLE screen retains the latest scan outcome independently of generic
worker status: scanning duration, completed matching-device count (including zero),
native failure, or cancellation reason. A completed zero-result scan means no
matching advertisements were collected; it does not establish QCY compatibility.
Accepted commands clear submission text, and worker completion clears a previous
busy rejection. Navigation restarts repaint polling with a new epoch without
closing an opted-in background session. Numeric command start/completion counters
and `BLE_DEMO scan_started`, `scan_completed matching_count=…`, `scan_failed`, and
`scan_cancelled reason=…` markers support correlation without peer/payload logging.
