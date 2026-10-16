# btleplug 0.13.5 local Android ABI fix

This is the unmodified crates.io 0.13.5 archive except the four files in
../patches/android-service-addressing.patch. Version and all dependencies remain
unchanged. Upstream license and crate metadata are retained.

Archive SHA-256: 3d836502de731602ea87cbb307065e8feaed4fd10f0855a75b0b87944ee011dc.
Source commit: 1af8299c291f24fca68dfb6b7ef1b5c0475892e1.

Reproduce into a separate directory:

    sh src/packages/ble/prepare-vendor.sh /path/to/verification
    diff -qr src/packages/ble/vendor/btleplug-0.13.5 /path/to/verification/btleplug-0.13.5

BTLEPLUG_ARCHIVE may point to an existing archive; its checksum is still required.
prepare-android.sh uses this identical preparation for APK Java. Never combine
registry Rust with the patched Java ABI, or patched Rust with unpatched Java.
The root btleplug dependency must select this path; independent probe manifests
must do the same. API parameter order is service UUID then characteristic UUID
(and descriptor UUID for descriptors); no shared mutable selector is involved.

Notification callbacks snapshot the service as well as the value; Rust obtains
service UUID from that snapshot rather than searching the first characteristic
UUID in the discovered-service cache. Responses validate both service and
characteristic. The CCCD completion callback is installed before native write.

The Java scanner forwards native errors to the gpui-mobile companion class and
forwards batch results. This narrow addition requires the existing Ble.java
companion in APK builds. No iOS/CoreBluetooth source was modified.

Regression runner: sh src/packages/ble/tests/android-service-addressing.sh.
It compiles actual patched production code against Android API34, then tests the
real Peripheral command queue with recording native-boundary objects. This is
not physical BLE evidence. Multiple *service instances sharing the same service
UUID* are outside btleplug's UUID-based public identity; distinct service UUIDs
sharing characteristic UUIDs are supported by this patch.
