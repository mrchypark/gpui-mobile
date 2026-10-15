//! Mobile event adapter and native Ely viewport; no network or backend UI dependency.
mod mobile;
use ely_gpui_component::{
    Assets,
    buttons::Button,
    canvas::InfiniteCanvas,
    forms::{Input, TextInput},
};
use gpui::{
    App, Bounds, Context, Entity, IntoElement, Render, Window, WindowOptions, canvas, div, point,
    prelude::*, px, size,
};
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
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
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
                    .child(Input::new(&self.input))
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
                    .child("Floating keyboard focus avoidance is not implemented."),
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
    #[cfg(debug_assertions)]
    {
        mobile::self_check();
        eprintln!("ELY_ADAPTER_SELFCHECK PASS");
    }
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
        let root = cx.new(|cx| ElyRoot {
            mobile: MobileViewport::default(),
            route: route.clone(),
            input: cx.new(|cx| TextInput::new(window, cx).placeholder("Tap to edit a map label")),
            events: 0,
            resets: 0,
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
        root
    })
    .expect("open Ely viewport window");
}
