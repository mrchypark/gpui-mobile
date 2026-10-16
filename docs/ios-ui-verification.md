# Ely iOS UI verification — 2026-10-15

Partial verification of the running Ely app from commit
`7253295c4e8f35ac756fc2b9a2f3cc0a590a060d`. This is not an all-UI-pass report.
The parent operated the unlocked Simulator; the evidence worker only read logs
and screenshots. GPUI remains `1a28cff4b409169bac058bca40dfbfeb7621d19b`,
Ely `7f6a3f91ceb6e6f6c2dbf4e43d49e01201e384cd`, and wgpu `29.0.4`.

## Pre-fix evidence scope

The developer restart at 22:24 KST produced PID 58295 and durable capture at
`work/ely-integration-evidence/ui-verification-stderr.log` under the enclosing
task directory (outside this repository). Adjacent `ui-verification-stdout.log`
is the stdout destination. The earlier `runtime-ios-keyboard-final.log` stopped
updating and must not be used as evidence of these interactions.

Both `ELY_FLOATING_KEYBOARD_SELFCHECK PASS` and `ELY_ADAPTER_SELFCHECK PASS`
appear at startup. These are executed deterministic assertions, separate from
the actual UI observations below. Log line references describe the inspected
first 69 lines; later appended events are outside this snapshot.

Screenshots are under the enclosing task's `outputs/ios-ui-verification/`:

- `01-button.png`: counter 1, pan 0,0, zoom 1.00, commits 0.
- `02-canvas-drag.png`: counter 1, pan -100,-50, zoom 1.00, commits 2;
  the text input has a blue focus outline.
- `03-resume-button.png`: counter 2, pan 0,0, zoom 1.00, commits 5;
  the input shows its caret and unchanged placeholder.

## Observed results

| Interaction | Actual evidence | Result and boundary |
| --- | --- | --- |
| Outside reset button | Lines 12–14: touch ID 0 uses `raw=false`, followed by `ELY_CONTROL reset=1`; first screenshot shows counter 1. | Native button delivery passed, counter 0 → 1. |
| Canvas tap | Lines 17–20: ID 2 uses `raw=true`; Ended changes commits 0 → 1. | Canvas adapter receives the tap and commits once. |
| Canvas drag | Lines 21–24: ID 3 starts at 173,420 and ends at 273,470; pan becomes -100,-50 and commits 1 → 2. Second screenshot agrees. | Final-position drag handling passed. This sequence has no Moved sample; later ID 5 does include Moved (lines 27–32). |
| Stationary two-contact gesture | Lines 33–40: IDs 6 and 7 overlap, both `raw=true`, then end; commits remain 3, zoom remains 1.0. | Multi-contact delivery and edit-preview cancellation/no extra commit observed. This does **not** verify pinch zoom. |
| Background/resume | Lines 57–64: resign-active, `Lifecycle(Paused)`, background, `Lifecycle(Resumed)`, active. Lines 65–67: next button touch uses `raw=false` and emits `reset=2`; third screenshot agrees. | Pause/resume and subsequent button responsiveness passed. All contacts had already ended. |
| Text field focus | Parent observed blue outline/caret; second and third screenshots corroborate. | Visual focus observed. Text entry and software keyboard activation failed on this pre-fix binary. |

Additional attempted gestures changed pan/commits through ordinary single-touch
sequences (IDs 8 and 11, ending at commits 5). They are not pinch evidence.
A second stationary pair (IDs 9 and 10) also left zoom and commits unchanged.

## Unverified interactions and limitations

- **Pinch zoom:** CUA did not support holding the required modifier through a
  drag. Concurrent click/drag requests serialized and produced single-touch
  movement rather than a moving two-contact gesture. Option-click produced
  stationary contacts only. Zoom stayed 1.0; no pinch success is claimed.
- **Control-area scroll:** IDs 4 and 13 correctly routed `raw=false`, but their
  drags contained Started/Ended without Moved; observed scroll position did not
  change. Scroll behavior remains unverified, not passed.
- **Pre-fix text/keyboard:** typing and software-keyboard toggle attempts did not enter
  text or display the keyboard. Volta's `FocusGained`/`FocusLost` native keyboard
  hook change was initially blocked by 564 MiB free disk. The later successful
  build and software-keyboard verification are recorded below.
- **Floating keyboard movement/resize/hide:** real UI behavior remains unverified.
  Passing startup geometry assertions do not establish this interaction.
- **Active-contact cancellation:** Home/background was invoked after release.
  Cancellation during an active touch, surface loss, and lifecycle stress were
  not tested. Pause/resume success must not be generalized to these cases.
- **Android runtime:** no connected adb device or SDK emulator was available;
  the verified APK build/signature is separate evidence.

## Focus-hook build and actual UI follow-up

After disk recovery, the same pinned dependencies and commit 7253295 plus the
local `src/ios/window.rs` focus hook were built with
`CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 sh examples/ely-integration/ios-simulator.sh --no-run`.
Compile/link/sign passed in 4m15s; strict codesign verification passed. Developer
install/launch at 23:12 KST produced PID 11799. Both startup selfchecks passed.
The previous app and logs were preserved. Existing dead-code and large
`__eh_frame` compact-unwind warnings remain; neither blocked the build.

Build/source proof under the enclosing task's `work/ely-integration-evidence/`:

- `ui-focus-fix.build.log`, SHA-256 `b886eb3fb3ab9795e6c3da9ad477e4496c60a5d68c806138c53de262aae6ae3e`.
- `ui-focus-fix.source.sha256` records the exact core window, app Rust files,
  manifest, lockfile and build script; all matched after the build.
- Core window source SHA-256 `a24fe548a1e1d896e14f9adb96226996230d4e4039c4464a7bd9328e57062b5b`.
- Executable SHA-256 `f92a7178ea3810d5605803f9300a91bfe0f48f1becf90fe07993a55e6e53d961`, also in
  `ui-focus-fix.artifacts.sha256`.
- Live capture: `ui-focus-fix.stderr.log` and `ui-focus-fix.stdout.log`.
  Saved UI trace: `outputs/ios-ui-verification/native-trace-focus-fix.log`.

The parent performed the following actual CUA interactions; the evidence worker
did not send native input. Screenshots below are in `outputs/ios-ui-verification/`.

| Interaction | Evidence and result |
| --- | --- |
| Software keyboard show and entry | Simulator software-keyboard toggle displayed the keyboard. After 30 Delete actions cleared prior characters, AX clicks on e, l, y produced exact `ely`, visible in `05-focus-fix-ely-keyboard.png`. Native Insets reported keyboard origin (0,566), size 402×308, visible=true. |
| Outside button while keyboard shown | `ELY_CONTROL reset=1` follows raw=false touch delivery. The button does not blur the input; this is not evidence of blur-triggered keyboard dismissal. |
| Canvas with keyboard shown | Raw canvas tap commits once; following drag changes pan to -75,-50 and commits to 2, with zoom 1.0. |
| Software keyboard hide | Simulator toggle produces keyboard=None, visible=false. `06-focus-fix-hidden-regression.png` shows `ely`, pan -75,-50 and commits 2 preserved. |
| Home and icon return | Final pause/resume preserves `ely`, pan -75,-50 and commits 2, captured in `07-focus-fix-resumed.png`; refreshed trace records Paused/Resumed. Contacts were released before backgrounding. |

Control-area drag still has raw=false Started/Ended without Moved and no observed
scroll change: scrolling remains unverified. Hardware `app.typeText` produced
Option-like glyphs (`´¬¥`) rather than the requested exact text; modifier state
is a suspected cause, not established. Exact hardware entry, automatic keyboard
appearance without Simulator toggle, blur dismissal, floating-keyboard avoidance,
pinch zoom, and active-contact cancellation remain unverified. The earlier
stationary multi-contact result does not close those gaps.

Handoff: **빌드·화면 조작 종료**. No build or UI work remains active; parent reported
9.2 GiB free at handoff. The generated target is retained for inexpensive
reproduction. No additional tests or target cleanup were performed.

## Individual hardware-key follow-up — 2026-10-16

**PASS for CUA individual key delivery:** the parent cleared the field with 25
`app.pressKey('BackSpace')` calls, then separately pressed `a`, `b`, `c`, `1`,
`2`, `3`. The field shows exact `abc123` in
`outputs/ios-ui-verification/09-hardware-abc123.png` under the enclosing task
directory. This verifies tool-driven individual hardware-key input in Simulator;
it is not a claim of human typing on a physical keyboard.

The same persisted native executable was used: SHA-256
`f92a7178ea3810d5605803f9300a91bfe0f48f1becf90fe07993a55e6e53d961`;
local focus-fix commit `a08990b`, remote baseline `7253295`. No rebuild was needed.

Bulk `app.typeText('abc123')` still produced `å∫ç123`. Its cause remains unresolved,
but the difference from successful individual keys and software-keyboard AX input
points to the bulk tool/Simulator path. Source inspection found that UIKit's
`insertText:` string is passed unchanged to Ely. No backend change is justified by
the bulk result alone. This supersedes only the earlier exact-hardware-entry gap;
the other dated verification limits above remain unchanged.


## Fresh patched-build UI follow-up — 2026-10-16

Evidence directory: enclosing workspace `outputs/mobile-fixes-build-20261016/`.
The new executable SHA-256 is
`28f154b4b794a0996d03321567e16b3e52c08b9e3ad40d706988d2452f2c6d63`.
All 12 builder source hashes matched at final audit. Build exit 0, signing verification,
and fresh adapter/floating-keyboard self-check markers passed. Runtime identity:
iPhone 16e / iOS 26.3, UDID `1CBC5201-CBB1-42D1-9DC2-CDAA33E624DC`, PID 56948.

- `01-fresh-render.png`: newly rendered app, before input.
- User manually clicked the field; parent observed focus, active Korean input-method
  characters, BackSpace clearing, and individual numeric keys producing exact `123`.
- `02-input123.png` at 11:14:55 KST directly shows `123` and the Korean software
  keyboard. Log geometry is logical origin (0,536), size 390×308, visible=true.
  This verifies toggle-associated keyboard display/inset delivery, not automatic
  appearance from focus alone. Earlier CUA imagery lagged the captured frame;
  that discrepancy is not a demonstrated backend layout defect.
- Parent used AX Home, then the ElyIntegration icon. Logs record Paused after
  11:16:14 and Resumed before active at 11:17:25. Text `123` survived. Parent then
  pressed software-keyboard AX digit 4; `03-resume-softkey1234.png` directly shows
  `1234`, keyboard visible, clicks0, pan0,0, commits0. Software-keyboard input and
  post-release lifecycle recovery pass; no active contacts were involved.

CUA coordinate drag remains blocked by `noWindowsAvailable`. Actual scrolling,
pinch, partial-pointer cancellation, blur dismissal and floating-keyboard behavior
remain unverified. New adapter cancellation/inset self-checks passed but do not
replace these runtime checks. Prior dated evidence above remains historical.
No source fix was indicated by these input observations. Reviewer captured only
requested screenshots; parent owned interaction and builder owns its app cleanup.
