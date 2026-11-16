# GPUI compatibility

See the canonical [mobile compatibility and validation record](compatibility.md) for dependency identities, application assets, current results, commands, and limitations.

## Text input configuration

The vendored `gpui-fast` adds `keyboard_type` and `secure` to `TextInputConfiguration` and signed numeric variants to `KeyboardType`. These are source-breaking changes for exhaustive struct literals and enum matches. Construct configurations with `..Default::default()` where appropriate, and handle the signed variants when matching keyboard types. The Git revision must include this API before an application uses it.

The `secure` setting requests platform password input traits. It does not prevent an application from exposing or logging its own text. Android and iOS keyboard behavior must be verified with the target keyboard and device configuration.
