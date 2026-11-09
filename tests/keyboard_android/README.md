# Android keyboard boundary regressions

`KeyboardRegression` exercises the production GpuiKeyboard.onCreateInputConnection and its real Android BaseInputConnection/Editable composing spans. A tiny test-only JNI sink supplies fixed editor identity and records outgoing messages. It does not load GPUI or impersonate a real IME, and makes no claim about native GPUI delivery or physical Korean keyboard behavior. The production class is compiled unchanged, not copied/reimplemented in the test.

Checks: forwarded dead accent is declined without an exception/commit, next ordinary and supplementary scalars commit exactly; committed prefix A plus composed 한 rejects both surrounding-delete forms without modifying or republishing the mark; Hangul composing replacements and later commit still work; ordinary surrounding deletion after commit still reaches JNI once.

After obtaining the parent's build/runtime slot:

```sh
sh tests/keyboard_android/build.sh /absolute/new-output-directory
adb -s OWNED_SERIAL install /absolute/new-output-directory/keyboard-regression.apk
adb -s OWNED_SERIAL shell am instrument -w -r dev.gpui.mobile.keyboardregression/dev.gpui.mobile.KeyboardRegression
```

Require the PASS stream and INSTRUMENTATION_CODE -1. Failure returns code0 plus stack. A successful adb shell exit alone is not a test PASS. The fixture finishes its own Activity. Parent removes only the newly installed `dev.gpui.mobile.keyboardregression` test package when finished, preserving output evidence. No existing app data is modified. The isolated test signing key has no production use.

`KEYBOARD_SOURCE` may select an original GpuiKeyboard.java to demonstrate before-failure with the same fixture; default always builds current production. Before-fix code fails at dead-key scalar conversion; testing the composing-delete failure separately requires bypassing only that earlier test stage, not changing production. Build script records and rechecks exact inputs. Do not run this fixture during another worker's device/UI ownership.
