# Upstream sync: 5fd54f4

This port compares [the upstream snapshot](https://github.com/itsbalamurali/gpui-mobile/commit/5fd54f4)
with fork base `28abfec`. Both descend from `c7cab3a`. It keeps the fork's
GPUI Fast dependency, window handles, IME ownership, and mobile event routing.
The upstream branch was not merged wholesale.

| Upstream change | Disposition on the Fast fork |
| --- | --- |
| `bef6363` / `2f5e4cf` — hover and cancellation | Existing cancellation remains. iOS now clears synthetic hover after the contact or fling ends; Android drops move events without a live gesture. Android hover cleanup remains separate. |
| `b5fed30` — IME, file picking, native gestures | Kept the fork's IME owners and file pickers. Added iOS attributed marked text through the existing proxy and UTF-16 selection path. iOS now offers a legacy-compatible drag to GPUI before scrolling, as the fork already does on Android. RawOnly delivery is unchanged. |
| `4504783` — Metal layer contents | Adopted top-left gravity so the previous drawable is not stretched during a UIKit resize animation. |
| `3de4bbe` / `fa46ab4` — idle frames and vsync | The fork already uses demand-driven iOS frames and Android frame pacing. |
| `2ce11e6` — iOS window deregistration | The fork already removes its registered handle on drop. |
| `41f6693` — registry dependencies | Kept GPUI Fast instead of switching to gpui-pre. |
| `3ffd1b6` — software Backspace repeat | Not ported; it relies on a private UIKit selector. |
| `5abbefa` / `3324d66` — UIKit embedding and an Activity-independent Android host | Not ported; both require separate integration with the fork's lifecycle. The existing Android host now releases a surface before recreating it on the same native window and forces a repaint after attachment. |
| `106347e` / `8bad581` — Android fling tuning, host assets, fling-axis guard | Not wired into the fork's input path. Its legacy path cancels an existing fling on contact; replacing scroll physics and changing raw-contact delivery need separate verification. |

## Verification

- The locked Ely integration example passed `cargo check` for Android ARM64
  and the ARM64 iOS simulator.
- Android ARM64 emulator: both trim handles moved. After Home and relaunch,
  the renderer log showed the surface being released and replaced, and the
  gallery drew the same edited range (`27.5–78.0 s`).
- iPad simulator: XCTest verified both trim handles and, in a separate run,
  live portrait-to-landscape rotation with checkbox state preserved.
- The Android test covered release followed by attachment. It did not force
  reattachment to an already configured native window. Attributed IME
  composition and RawOnly map gestures were not exercised on devices.

## Public Ely integration follow-up

The integrated example pins the public Ely revision containing touch image
cropping and keyboard-aware dialogs. The example installs the shared input
owner reporter by default. The platform supplies keyboard geometry; Ely
chooses the visible region and scrolls the focused field.

- The Android emulator passed three same-process Activity recreations and
  the rendering-surface insets instrumentation check. In the gallery, a
  narrow landscape dialog kept both fields and Save reachable with the
  software keyboard open. A test IME composed Korean in each field without
  mixing the edits.
- The iPad simulator passed finger crop movement and Korean composition
  across a focus change. Moving the floating keyboard over the old field
  location moved the dialog so the field and Save stayed visible.
- The Android BLE release APK contained the release native library and
  launched. A debuggable APK intentionally built with that release library
  also launched, without invoking the unavailable probe exports.
