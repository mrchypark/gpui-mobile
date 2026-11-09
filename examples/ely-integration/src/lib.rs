//! Mobile event adapter and native Ely viewport; no network or backend UI dependency.
mod floating_keyboard;
mod mobile;
use ely_gpui_component::{
    Assets,
    buttons::Button,
    canvas::InfiniteCanvas,
    forms::{Input, TextInput},
};
use gpui::{
    App, Bounds, Context, Entity, Focusable, IntoElement, Render, Window, WindowOptions, canvas,
    div, point, prelude::*, px, size,
};
use gpui_kit::component::button::Button as KitButton;
#[cfg(all(feature = "platform-smoke", any(target_os = "ios", target_os = "android")))]
mod platform_smoke;

use gpui_mobile::mobile_events::{
    MobileEvent, TouchDeliveryPolicy, set_mobile_event_handler, set_touch_delivery_policy,
};
use mobile::{GestureRoute, MobileViewport};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

struct ElyRoot {
    mobile: MobileViewport,
    route: Rc<RefCell<GestureRoute>>,
    input: Entity<TextInput>,
    second_input: Entity<TextInput>,
    controls_scroll: gpui::ScrollHandle,
    keyboard_reveal: Rc<RefCell<floating_keyboard::RevealState>>,
    _focus_subscriptions: Vec<gpui::Subscription>,
    events: usize,
    resets: usize,
}
impl Render for ElyRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let available = self
            .mobile
            .available(Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        let header = px(180.).min(available.size.height);
        let area = Bounds::new(
            available.origin + point(px(0.), header),
            size(
                available.size.width,
                (available.size.height - header).max(px(0.)),
            ),
        );
        let measure = self.route.clone();
        let scroll = self.controls_scroll.clone();
        let keyboard_reveal = self.keyboard_reveal.clone();
        let second_keyboard_reveal = keyboard_reveal.clone();
        let focus = self.input.read(cx).focus_handle(cx);
        let second_focus = self.second_input.read(cx).focus_handle(cx);
        let second_scroll = scroll.clone();
        let keyboard = if self.mobile.insets.keyboard_visible == Some(false) {
            None
        } else {
            self.mobile.insets.keyboard
        };
        let second_keyboard = keyboard.clone();
        div()
            .size_full()
            .relative()
            .bg(gpui::rgb(0xf2f4f8))
            .text_color(gpui::rgb(0x182838))
            .child(
                div()
                    .id("controls")
                    .absolute()
                    .left(available.origin.x)
                    .top(available.origin.y)
                    .w(available.size.width)
                    .h(header)
                    .overflow_y_scroll()
                    .track_scroll(&self.controls_scroll)
                    .child(div()
                    // Leave room to reveal a field above or below a floating keyboard.
                    .pt(px(180.))
                    .pb(px(180.))
                    .child(div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(KitButton::new("kit-reset").label("Kit: reset viewport").on_click(cx.listener(|view, _, _, cx| {
                        view.mobile.view = ely_gpui_component::canvas::Viewport::new(0., 0., 1.);
                        view.resets += 1;
                        cx.notify();
                    })))
                    .child(
                        Button::new("reset", format!("Reset viewport · clicks {}", self.resets))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.mobile.view =
                                    ely_gpui_component::canvas::Viewport::new(0., 0., 1.);
                                view.resets += 1;
                                eprintln!("ELY_CONTROL reset={}", view.resets);
                                cx.notify();
                            })),
                    )
                    .child(div().relative().flex().flex_col().gap_2().flex_shrink_0()
                        .child(div().relative()
                            .child(Input::new(&self.input))
                            .child(canvas(
                            |bounds, _, _| bounds,
                            move |_, field, window, _| {
                                if focus.is_focused(window)
                                    && keyboard_reveal.borrow_mut().reveal(
                                        &scroll,
                                        Some(0),
                                        field,
                                        keyboard,
                                    )
                                {
                                    window.request_animation_frame();
                                }
                            },
                        ).absolute().top_0().left_0().size_full()))
                        .child(div().relative()
                            .child(Input::new(&self.second_input))
                            .child(canvas(
                            |bounds, _, _| bounds,
                            move |_, field, window, _| {
                                if second_focus.is_focused(window)
                                    && second_keyboard_reveal.borrow_mut().reveal(
                                        &second_scroll,
                                        Some(1),
                                        field,
                                        second_keyboard,
                                    )
                                {
                                    window.request_animation_frame();
                                }
                            },
                        ).absolute().top_0().left_0().size_full())))
                    .child(format!(
                        "OS {} · zoom {:.2} · pan {:.0},{:.0} · commits {}",
                        self.events,
                        self.mobile.view.zoom,
                        self.mobile.view.x,
                        self.mobile.view.y,
                        self.mobile.commits
                    ))
                    .child("Canvas: drag / pinch. Controls: native tap / scroll.")
                    .child("Scroll this control area; gestures stay with their start region.")
                    .child("Focused input scrolls clear of keyboard overlap."))),
            )
            .child(
                div()
                    .absolute()
                    .left(area.origin.x)
                    .top(area.origin.y)
                    .w(area.size.width)
                    .h(area.size.height)
                    .overflow_hidden()
                    .child(InfiniteCanvas::new("mobile-map", self.mobile.view).rulers())
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                measure.borrow_mut().painted = bounds;
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    ),
            )
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn run() {
    #[cfg(target_os = "ios")]
    {
        gpui_ios_register_app();
        gpui_ios_run_ely();
    }
    #[cfg(target_os = "android")]
    {
        let platform = gpui_mobile::android::jni::shared_platform()
            .expect("init_platform before run")
            .into_rc();
        gpui::Application::with_platform(platform)
            .with_assets(Assets)
            .run(open_window);
    }
}
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_register_app() {
    gpui_mobile::ios::ffi::set_app_callback(Box::new(open_window));
}
#[cfg(target_os = "ios")]
#[unsafe(no_mangle)]
pub extern "C" fn gpui_ios_run_ely() {
    gpui_mobile::ios::ffi::run_app_with_assets(Assets);
}

fn open_window(cx: &mut App) {
    #[cfg(all(target_os = "android", feature = "lifecycle-stress"))]
    {
        let timer = cx.background_executor().timer(std::time::Duration::from_secs(4));
        cx.foreground_executor().spawn(async move { timer.await }).detach();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None::<std::task::Waker>));
        let wake = slot.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(4));
            if let Some(waker) = wake.lock().unwrap().take() {
                waker.wake();
            }
        });
        cx.foreground_executor()
            .spawn(std::future::poll_fn(move |cx| {
                *slot.lock().unwrap() = Some(cx.waker().clone());
                std::task::Poll::<()>::Pending
            }))
            .detach();
    }
    #[cfg(all(feature = "platform-smoke", any(target_os = "ios", target_os = "android")))]
    platform_smoke::run();
    #[cfg(all(feature = "platform-extended", any(target_os = "ios", target_os = "android")))]
    platform_smoke::run_extended();
    #[cfg(all(feature = "platform-device", target_os = "android"))]
    platform_smoke::run_device();
    #[cfg(debug_assertions)]
    {
        mobile::self_check();
        floating_keyboard::self_check();
        eprintln!("ELY_FLOATING_KEYBOARD_SELFCHECK PASS");
        eprintln!("ELY_ADAPTER_SELFCHECK PASS");
    }
    gpui_kit::init(cx);
    ely_gpui_component::init(cx).expect("initialize Ely assets/fonts/theme");
    set_touch_delivery_policy(TouchDeliveryPolicy::LegacyCompatible);
    cx.open_window(WindowOptions::default(), |window, cx| {
        #[cfg(target_os = "ios")]
        let owner = gpui_mobile::ios::ffi::gpui_ios_get_window() as u64;
        #[cfg(target_os = "android")]
        let owner = gpui_mobile::android::jni::platform()
            .unwrap()
            .primary_window()
            .unwrap()
            .id();
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        let owner = 0;
        let route = Rc::new(RefCell::new(GestureRoute::new(owner)));
        let root = cx.new(|cx| {
            let input = cx.new(|cx| {
                TextInput::new(window, cx)
                    .placeholder("Tap to edit a map label")
                    .input_owner_observer(|owner| {
                        gpui_mobile::mobile_events::report_text_input_owner(
                            gpui_mobile::mobile_events::TextInputOwner {
                                window: owner.window.as_u64(),
                                editor: owner.editor.as_u64(),
                                activation: owner.activation,
                            },
                            owner.focused,
                            owner.disabled,
                        );
                    })
            });
            let second_input = cx.new(|cx| {
                TextInput::new(window, cx)
                    .placeholder("Tap to edit a second map label")
                    .input_owner_observer(|owner| {
                        gpui_mobile::mobile_events::report_text_input_owner(
                            gpui_mobile::mobile_events::TextInputOwner {
                                window: owner.window.as_u64(),
                                editor: owner.editor.as_u64(),
                                activation: owner.activation,
                            },
                            owner.focused,
                            owner.disabled,
                        );
                    })
            });
            let focus = input.read(cx).focus_handle(cx);
            let second_focus = second_input.read(cx).focus_handle(cx);
            let keyboard_reveal =
                Rc::new(RefCell::new(floating_keyboard::RevealState::default()));
            let first_blur_reveal = keyboard_reveal.clone();
            let second_blur_reveal = keyboard_reveal.clone();
            let subscriptions = vec![
                cx.on_focus(&focus, window, |_, _, cx| cx.notify()),
                cx.on_blur(&focus, window, move |_, _, cx| {
                    first_blur_reveal.borrow_mut().focus_lost();
                    cx.notify();
                }),
                cx.on_focus(&second_focus, window, |_, _, cx| cx.notify()),
                cx.on_blur(&second_focus, window, move |_, _, cx| {
                    second_blur_reveal.borrow_mut().focus_lost();
                    cx.notify();
                }),
            ];
            let controls_scroll = gpui::ScrollHandle::new();
            controls_scroll.set_offset(point(px(0.), px(-180.)));
            ElyRoot {
                mobile: MobileViewport::default(),
                route: route.clone(),
                input,
                second_input,
                controls_scroll,
                keyboard_reveal,
                _focus_subscriptions: subscriptions,
                events: 0,
                resets: 0,
            }
        });
        let weak = root.downgrade();
        let app = cx.to_async();
        let executor = cx.foreground_executor().clone();
        let pending = Rc::new(RefCell::new(VecDeque::new()));
        let scheduled = Rc::new(Cell::new(false));
        set_mobile_event_handler(Some(Box::new(move |window, event| {
            // Observer runs before synthesis. Never borrow App or pump input here.
            let (raw, painted) = {
                let mut route = route.borrow_mut();
                (route.route(window, event), route.painted)
            };
            set_touch_delivery_policy(if raw {
                TouchDeliveryPolicy::RawOnly
            } else {
                TouchDeliveryPolicy::LegacyCompatible
            });
            if window != owner {
                return;
            }
            eprintln!("ELY_ROUTE window={window} raw={raw} event={event:?}");
            // Insets/lifecycle always reach the model; outside raw contacts do not.
            if !raw && matches!(event, MobileEvent::Touch { .. } | MobileEvent::Pinch { .. }) {
                return;
            }
            pending.borrow_mut().push_back((painted, event.clone()));
            if scheduled.replace(true) {
                return;
            }
            let (pending, scheduled, weak, mut app) = (
                pending.clone(),
                scheduled.clone(),
                weak.clone(),
                app.clone(),
            );
            executor
                .spawn(async move {
                    let events: Vec<_> = pending.borrow_mut().drain(..).collect();
                    scheduled.set(false);
                    let _ = weak.update(&mut app, |root, cx| {
                        for (painted, event) in events {
                            root.mobile.set_canvas(painted);
                            root.mobile.event(&event);
                            root.events += 1;
                            eprintln!(
                                "ELY_MOBILE {event:?} => viewport={:?} commits={}",
                                root.mobile.view, root.mobile.commits
                            );
                        }
                        cx.notify();
                    });
                })
                .detach();
        })));
        cx.new(|cx| gpui_kit::component::Root::new(root, window, cx))
    })
    .expect("open Ely viewport window");
}

/// NativeActivity enters here; initialize JNI once, then reuse its platform and Ely assets.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: android_activity::AndroidApp) {
    gpui_mobile::android::jni::install_panic_hook();
    let _platform_session = gpui_mobile::android::jni::init_platform(&app);
    run();
}
