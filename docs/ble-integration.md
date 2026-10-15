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
  are included only in BLE mode. No third-party source is vendored into Git.

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
- btleplug 0.13.5 Android `ScanCallback` does not forward `onScanFailed`; use an
  application discovery deadline and report absence/timeout, not scan success.
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
