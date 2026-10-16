# Mobile event integration

Register `gpui_mobile::mobile_events::set_mobile_event_handler` on the platform UI
thread before opening windows. The callback receives a window identifier and an
owned-queue-backed observation. Nested events follow the entire current batch;
callbacks may clear or replace themselves. Window identifiers are lifetime-scoped.

For Ely-owned gesture delivery, call
`set_touch_delivery_policy(TouchDeliveryPolicy::RawOnly)` on that same UI thread
before accepting touches. This is thread-wide, shared by every window on that UI
thread, not per-window. The default is `LegacyCompatible`. RawOnly preserves raw
touch/pinch, insets, lifecycle, and text but suppresses backend synthetic mouse,
scroll, and momentum delivery. Both backends invoke the raw observer before checking
the synthesis policy. An app may hit-test the first Started and select the policy
synchronously there, keeping it latched through all Ended/Cancelled events until
the next gesture's first Started. Additional pointers must inherit that choice.
Do not pump nested native input from this routing callback: queued observers are
not synchronous with nested backend synthesis. The app
owns hit-testing, capture, cancellation, zoom limits, and layout; choose either
native pinch observations or recognition from raw pointers to avoid double zoom.
Core has no Ely dependency and no GPUI source/revision change.

## Contract and native wiring

- Touch uses actual GPUI TouchEvent/TouchId/TouchPhase types. Native contact IDs map
  to fresh IDs per lifetime. Cancel never produces click/up/fling. Invalid terminal
  coordinates use the last valid position and release pointer/pinch state.
- Coordinates are window-local logical pixels: UIKit points, Android physical
  coordinates divided by current density. Insets are independent observations;
  apps must not blindly add safe area and IME occlusion.
- UIKit raw touches and UIPinchGestureRecognizer are wired without cancelling raw
  delivery. Android emits measured two-pointer span ratios; a third pointer cancels
  that span stream. Android JNI preserves ACTION_CANCEL, even with no pointers or
  over native views.
- UIKit keyboard frame/hide observers convert screen geometry into local view
  coordinates and intersect bounds. Layout reports safe area.
- Android GpuiInsets.java uses WindowInsetsCompat to report systemBars/displayCutout
  separately from IME, relative to the NativeActivity SurfaceView. JNI queues the
  payload onto the GPUI thread. Keyboard visibility and geometry are independent;
  a visible floating IME can have no reported edge rectangle. NativeActivity uses
  adjustResize. The content rectangle is only fallback until native insets arrive.
- App lifecycle helpers are IosWindowInner::notify_active_status_change(bool) and
  AndroidWindow::notify_app_lifecycle(bool). Pause cancels active input before its
  lifecycle event. WindowVisibility follows availability/activity. App pause is
  distinct from system sleep; Android focus alone does not imply pause.
- UIKit didMoveToWindow/layout detect drawable detach/zero size and reattachment.
  Surface replacement retains the GPU context and atlas. Android init_window and
  term_window independently report surface availability. Rendering is gated by both
  app activity and surface availability.

## Callback ownership

The iOS PlatformWindow wrapper owns Rc<IosWindowInner>. A UI-thread weak registry
uses monotonic opaque handles, and every native dispatch upgrades to Rc before
calling app code. Registry borrows end before callbacks. Closing a window marks it
closed, unregisters its handle, zeroes native ivars and removes observers. Active
callbacks retain storage; renderer destruction precedes native view releases.
Callback slots release RefCell borrows before invocation and restore only when not
closed or replaced. Text input uses the same FIFO callback mechanism with owned
strings so focus changes and nested text cannot borrow-panic across UIKit.

## Verification and limits

The standalone simulator harness imports the production mobile_events.rs and
mobile_dispatch.rs and links the existing actual GPUI simulator rlib. It does not
substitute GPUI types. Run the signed binary from the installed simulator app data
container's tmp directory; direct execution from the host Documents path hung in
dyld. The old binary, diagnostic sample, and logs are preserved in work/.

All 13 event-state, ownership, text and FIFO regressions passed together
in the simulator. The regressions exercise the actual text dispatcher clearing,
replacement and nested owned strings, and actual mobile Touch/Pinch batching before
nested cancellation/pause. Latest execution evidence is
`work/mobile-backend-simulator-tests.log` (relative to the project workspace).

Native compile/link/launch results and exact source identities are recorded in
[compatibility](compatibility.md). These unit tests do not establish physical
multi-touch, floating IME behavior, or real-device surface recreation. Actual UI
clicks, drags, pinches and keyboard interaction were not verified while the Mac
was locked.
