//! Packages demo screen — showcases all 12 gpui-mobile utility packages.

use std::cell::RefCell;

use gpui::{div, prelude::*, px, rgb};

use super::{Router, BLUE, GREEN, LIGHT_CARD_BG, LIGHT_DIVIDER, LIGHT_SUBTEXT, LIGHT_TEXT, MAUVE, PEACH, SURFACE0, SURFACE1, TEAL, TEXT, YELLOW};

// ── Thread-local packages-demo state ────────────────────────────────────────

/// All mutable state for the Packages demo screen, stored in a thread-local
/// instead of on `Router`.
#[derive(Default)]
pub(crate) struct PackagesState {
    pub last_picked_file: Option<String>,
    pub last_picked_image: Option<String>,
    pub camera_handle: Option<usize>,
    pub camera_preview_handle: Option<std::sync::Arc<gpui_mobile::platform_view::PlatformViewHandle>>,
    pub camera_direction: Option<gpui_mobile::packages::camera::CameraLensDirection>,
    pub camera_list: Option<String>,
    pub camera_busy: bool,
    pub camera_status: Option<String>,
    pub camera_previewing: bool,
    pub camera_recording: bool,
    pub perm_status: Option<String>,
    pub location_status: Option<String>,
    pub notif_status: Option<String>,
    pub notif_counter: i32,
    pub audio_status: Option<String>,
    pub video_status: Option<String>,
    pub url_status: Option<String>,
}

thread_local! {
    pub(crate) static PACKAGES_STATE: RefCell<PackagesState> = RefCell::new(PackagesState::default());
}

struct PendingCamera(Option<gpui_mobile::packages::camera::CameraHandle>);

impl Drop for PendingCamera {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            std::thread::spawn(move || {
                let _ = gpui_mobile::packages::camera::dispose(handle);
            });
        }
    }
}

// Called from a foreground task, outside any Router/PackagesState borrow.
// iOS pickers service UIKit's run loop; Android pickers block a worker.
async fn run_picker<T: Send + 'static>(
    executor: &gpui::BackgroundExecutor,
    operation: impl FnOnce() -> T + Send + 'static,
) -> T {
    #[cfg(target_os = "ios")]
    { let _ = executor; operation() }
    #[cfg(not(target_os = "ios"))]
    { executor.spawn(async move { operation() }).await }
}

// Camera session calls can block for tens of seconds. Keep one native operation
// in flight at a time and apply its result on the UI thread.
fn spawn_camera_operation<T: Send + 'static>(
    cx: &mut gpui::Context<Router>,
    busy_status: &'static str,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
    apply: impl FnOnce(&mut PackagesState, Result<T, String>) + 'static,
) {
    let started = PACKAGES_STATE.with(|s| {
        let mut state = s.borrow_mut();
        if state.camera_busy {
            false
        } else {
            state.camera_busy = true;
            state.camera_status = Some(busy_status.into());
            true
        }
    });
    if !started {
        PACKAGES_STATE.with(|s| s.borrow_mut().camera_status = Some("Camera is busy; try again".into()));
        cx.notify();
        return;
    }
    cx.notify();

    cx.spawn(async move |this, cx| {
        let result = cx.background_executor().spawn(async move { operation() }).await;
        let updated = this.update(cx, move |_this, cx| {
            PACKAGES_STATE.with(|s| {
                let mut state = s.borrow_mut();
                state.camera_busy = false;
                apply(&mut state, result);
            });
            cx.notify();
        });
        if updated.is_err() {
            PACKAGES_STATE.with(|s| s.borrow_mut().camera_busy = false);
        }
    })
    .detach();
}

fn spawn_camera_flash(
    cx: &mut gpui::Context<Router>,
    mode: gpui_mobile::packages::camera::FlashMode,
    success_status: &'static str,
) {
    if let Some(id) = PACKAGES_STATE.with(|s| s.borrow().camera_handle) {
        spawn_camera_operation(
            cx,
            "Setting flash...",
            move || {
                let handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                let result = gpui_mobile::packages::camera::set_flash_mode(&handle, mode);
                std::mem::forget(handle);
                result
            },
            move |state, result| {
                if state.camera_handle == Some(id) {
                    state.camera_status = Some(match result {
                        Ok(()) => success_status.into(),
                        Err(e) => format!("Error: {e}"),
                    });
                }
            },
        );
    }
}

/// Render the Packages demo screen.
pub fn render(router: &Router, cx: &mut gpui::Context<Router>) -> impl IntoElement {
    let dark_mode = router.dark_mode;
    let text_color = if dark_mode { TEXT } else { LIGHT_TEXT };
    let sub_text: u32 = if dark_mode { super::SUBTEXT } else { LIGHT_SUBTEXT };
    let card_bg = if dark_mode { SURFACE0 } else { LIGHT_CARD_BG };
    let divider_color = if dark_mode { SURFACE1 } else { LIGHT_DIVIDER };

    let mut root = div().flex().flex_col().flex_1().gap_4().px_4().py_6();

    // ── Device Info (native API, no JNI) ────────────────────────────────────
    root = root
        .child(section_header("Device Info", sub_text))
        .child({
            let info = gpui_mobile::packages::device_info::get_device_info();
            match info {
                Ok(di) => info_card(card_bg)
                    .child(kv_row("Model", &di.model, GREEN, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row("Manufacturer", &di.manufacturer, GREEN, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row("OS Version", &di.os_version, GREEN, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row("Device Name", &di.device_name, GREEN, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row(
                        "Physical Device",
                        if di.is_physical_device { "Yes" } else { "No" },
                        GREEN,
                        text_color,
                        sub_text,
                    ))
                    .into_any_element(),
                Err(e) => error_card(&e, card_bg, text_color).into_any_element(),
            }
        });

    // ── Path Provider (native API, no JNI) ──────────────────────────────────
    root = root
        .child(section_header("Path Provider", sub_text))
        .child({
            let tmp = gpui_mobile::packages::path_provider::temporary_directory();
            let docs = gpui_mobile::packages::path_provider::documents_directory();
            let cache = gpui_mobile::packages::path_provider::cache_directory();
            let support = gpui_mobile::packages::path_provider::support_directory();

            info_card(card_bg)
                .child(kv_row("Temp", &path_or_err(&tmp), MAUVE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row("Documents", &path_or_err(&docs), MAUVE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row("Cache", &path_or_err(&cache), MAUVE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row("Support", &path_or_err(&support), MAUVE, text_color, sub_text))
        });

    // ── Package Info (JNI) ──────────────────────────────────────────────────
    root = root
        .child(section_header("Package Info", sub_text))
        .child({
            let info = gpui_mobile::packages::package_info::get_package_info();
            match info {
                Ok(pi) => info_card(card_bg)
                    .child(kv_row("App Name", &pi.app_name, BLUE, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row("Package", &pi.package_name, BLUE, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row("Version", &pi.version, BLUE, text_color, sub_text))
                    .child(divider_line(divider_color))
                    .child(kv_row("Build", &pi.build_number, BLUE, text_color, sub_text))
                    .into_any_element(),
                Err(e) => error_card(&e, card_bg, text_color).into_any_element(),
            }
        });

    // ── Connectivity (JNI) ──────────────────────────────────────────────────
    root = root
        .child(section_header("Connectivity", sub_text))
        .child({
            let status = gpui_mobile::packages::connectivity::check_connectivity();
            let label = format!("{:?}", status);
            info_card(card_bg).child(kv_row("Status", &label, TEAL, text_color, sub_text))
        });

    // ── Network Info (JNI) ──────────────────────────────────────────────────
    root = root
        .child(section_header("Network Info", sub_text))
        .child({
            let info = gpui_mobile::packages::network_info::get_network_info();
            match info {
                Ok(ni) => info_card(card_bg)
                    .child(kv_row(
                        "WiFi Name",
                        ni.wifi_name.as_deref().unwrap_or("N/A"),
                        YELLOW,
                        text_color,
                        sub_text,
                    ))
                    .child(divider_line(divider_color))
                    .child(kv_row(
                        "WiFi BSSID",
                        ni.wifi_bssid.as_deref().unwrap_or("N/A"),
                        YELLOW,
                        text_color,
                        sub_text,
                    ))
                    .child(divider_line(divider_color))
                    .child(kv_row(
                        "WiFi IP",
                        ni.wifi_ip.as_deref().unwrap_or("N/A"),
                        YELLOW,
                        text_color,
                        sub_text,
                    ))
                    .into_any_element(),
                Err(e) => error_card(&e, card_bg, text_color).into_any_element(),
            }
        });

    // ── Shared Preferences (JNI) ────────────────────────────────────────────
    root = root
        .child(section_header("Shared Preferences", sub_text))
        .child({
            let prefs = gpui_mobile::packages::shared_preferences::SharedPreferences::instance();
            let key = "gpui_demo_counter";
            let current = prefs.get_int(key).unwrap_or(0);
            let _ = prefs.set_int(key, current + 1);
            info_card(card_bg)
                .child(kv_row("Demo Key", key, PEACH, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row(
                    "Value (increments each visit)",
                    &current.to_string(),
                    PEACH,
                    text_color,
                    sub_text,
                ))
        });

    // ── Vibration (JNI) ─────────────────────────────────────────────────────
    root = root
        .child(section_header("Vibration", sub_text))
        .child({
            let can = gpui_mobile::packages::vibration::can_vibrate();
            let mut card = info_card(card_bg).child(kv_row(
                "Can Vibrate",
                if can { "Yes" } else { "No" },
                PEACH,
                text_color,
                sub_text,
            ));

            if can {
                card = card.child(divider_line(divider_color)).child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(haptic_button("Light", BLUE, cx.listener(|_this, _, _, cx| {
                            let _ = gpui_mobile::packages::vibration::haptic_feedback(
                                gpui_mobile::packages::vibration::HapticFeedback::Light,
                            );
                            cx.notify();
                        })))
                        .child(haptic_button("Medium", GREEN, cx.listener(|_this, _, _, cx| {
                            let _ = gpui_mobile::packages::vibration::haptic_feedback(
                                gpui_mobile::packages::vibration::HapticFeedback::Medium,
                            );
                            cx.notify();
                        })))
                        .child(haptic_button("Heavy", MAUVE, cx.listener(|_this, _, _, cx| {
                            let _ = gpui_mobile::packages::vibration::haptic_feedback(
                                gpui_mobile::packages::vibration::HapticFeedback::Heavy,
                            );
                            cx.notify();
                        })))
                        .child(haptic_button("Success", TEAL, cx.listener(|_this, _, _, cx| {
                            let _ = gpui_mobile::packages::vibration::haptic_feedback(
                                gpui_mobile::packages::vibration::HapticFeedback::Success,
                            );
                            cx.notify();
                        }))),
                );
            }
            card
        });

    // ── URL Launcher (JNI) ──────────────────────────────────────────────────
    root = root
        .child(section_header("URL Launcher", sub_text))
        .child({
            let can = gpui_mobile::packages::url_launcher::can_launch_url("https://zed.dev");
            let status = PACKAGES_STATE.with(|s| s.borrow().url_status.clone().unwrap_or_else(|| "Not opened".into()));
            info_card(card_bg)
                .child(kv_row(
                    "Can open https://zed.dev",
                    &format!("{:?}", can),
                    BLUE,
                    text_color,
                    sub_text,
                ))
                .child(kv_row("Last launch", &status, BLUE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .p_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .px_4()
                                .py_2()
                                .rounded_lg()
                                .bg(rgb(BLUE))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(0x1e1e2e))
                                        .child("Open zed.dev"),
                                )
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|_this, _, _, cx| {
                                        cx.spawn(async |this, cx| {
                                            let result = cx.background_executor().spawn(async {
                                                gpui_mobile::packages::url_launcher::launch_url("https://zed.dev")
                                            }).await;
                                            let _ = this.update(cx, |_this, cx| {
                                                PACKAGES_STATE.with(|s| s.borrow_mut().url_status = Some(match result {
                                                    Ok(true) => "Opened".into(),
                                                    Ok(false) => "Not opened: no application accepted the URL".into(),
                                                    Err(error) => format!("Error: {error}"),
                                                }));
                                                cx.notify();
                                            });
                                        }).detach();
                                    }),
                                ),
                        ),
                )
        });

    // ── Battery ───────────────────────────────────────────────────────────────
    root = root
        .child(section_header("Battery", sub_text))
        .child({
            let bi = gpui_mobile::packages::battery::battery_info();
            info_card(card_bg)
                .child(kv_row("Level", &format!("{}%", bi.level), GREEN, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row("State", &format!("{:?}", bi.state), GREEN, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row(
                    "Battery Saver",
                    if bi.is_battery_save_mode { "On" } else { "Off" },
                    GREEN,
                    text_color,
                    sub_text,
                ))
        });

    // ── Sensors ───────────────────────────────────────────────────────────────
    root = root
        .child(section_header("Sensors", sub_text))
        .child({
            let avail = gpui_mobile::packages::sensors::available_sensors();
            let mut card = info_card(card_bg)
                .child(kv_row(
                    "Accelerometer",
                    if avail.accelerometer { "Available" } else { "N/A" },
                    YELLOW,
                    text_color,
                    sub_text,
                ))
                .child(divider_line(divider_color))
                .child(kv_row(
                    "Gyroscope",
                    if avail.gyroscope { "Available" } else { "N/A" },
                    YELLOW,
                    text_color,
                    sub_text,
                ))
                .child(divider_line(divider_color))
                .child(kv_row(
                    "Magnetometer",
                    if avail.magnetometer { "Available" } else { "N/A" },
                    YELLOW,
                    text_color,
                    sub_text,
                ))
                .child(divider_line(divider_color))
                .child(kv_row(
                    "Barometer",
                    if avail.barometer { "Available" } else { "N/A" },
                    YELLOW,
                    text_color,
                    sub_text,
                ));

            // Show live accelerometer reading if available
            if let Some(accel) = gpui_mobile::packages::sensors::accelerometer() {
                card = card
                    .child(divider_line(divider_color))
                    .child(kv_row(
                        "Accel (m/s²)",
                        &format!("x={:.1} y={:.1} z={:.1}", accel.x, accel.y, accel.z),
                        YELLOW,
                        text_color,
                        sub_text,
                    ));
            }
            card
        });

    // ── Share ─────────────────────────────────────────────────────────────────
    root = root
        .child(section_header("Share", sub_text))
        .child({
            info_card(card_bg)
                .child(
                    div()
                        .p_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .px_4()
                                .py_2()
                                .rounded_lg()
                                .bg(rgb(TEAL))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(rgb(0x1e1e2e))
                                        .child("Share \"Hello from GPUI!\""),
                                )
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|_this, _, _, cx| {
                                        let _ = gpui_mobile::packages::share::share_text(
                                            "Hello from GPUI!",
                                            Some("GPUI Demo"),
                                        );
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
        });

    // ── WebView ───────────────────────────────────────────────────────────────
    root = root
        .child(section_header("WebView", sub_text))
        .child({
            info_card(card_bg).child(
                div()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_4()
                            .py_3()
                            .rounded_lg()
                            .bg(rgb(MAUVE))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgb(0xFFFFFF))
                                    .child("Open In-App Browser"),
                            )
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.navigate_to(super::Screen::WebViewBrowser);
                                    cx.notify();
                                }),
                            ),
                    ),
            )
        });

    // ── File Selector ────────────────────────────────────────────────────────
    root = root
        .child(section_header("File Selector", sub_text))
        .child({
            let last_path = PACKAGES_STATE.with(|s| {
                s.borrow().last_picked_file.as_deref().unwrap_or("None").to_string()
            });
            info_card(card_bg)
                .child(kv_row("Last Picked", &last_path, BLUE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(
                            action_button("Pick File", BLUE, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        let opts = gpui_mobile::packages::file_selector::OpenFileOptions::default();
                                        gpui_mobile::packages::file_selector::open_file(&opts)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(f)) => state.last_picked_file = Some(f.name),
                                                Ok(None) => state.last_picked_file = Some("Cancelled".into()),
                                                Err(e) => state.last_picked_file = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Pick Files", GREEN, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        let opts = gpui_mobile::packages::file_selector::OpenFileOptions::default();
                                        gpui_mobile::packages::file_selector::open_files(&opts)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(files) => state.last_picked_file = Some(format!("{} files", files.len())),
                                                Err(e) => state.last_picked_file = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Pick Dir", TEAL, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        gpui_mobile::packages::file_selector::get_directory_path(None)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(d)) => state.last_picked_file = Some(d),
                                                Ok(None) => state.last_picked_file = Some("Cancelled".into()),
                                                Err(e) => state.last_picked_file = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        ),
                )
        });

    // ── Image Picker ─────────────────────────────────────────────────────────
    root = root
        .child(section_header("Image Picker", sub_text))
        .child({
            let last_image = PACKAGES_STATE.with(|s| {
                s.borrow().last_picked_image.as_deref().unwrap_or("None").to_string()
            });
            info_card(card_bg)
                .child(kv_row("Last Picked", &last_image, MAUVE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(
                            action_button("Gallery", MAUVE, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        let opts = gpui_mobile::packages::image_picker::ImagePickerOptions {
                                            source: gpui_mobile::packages::image_picker::ImageSource::Gallery,
                                            ..Default::default()
                                        };
                                        gpui_mobile::packages::image_picker::pick_image(&opts)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(f)) => state.last_picked_image = Some(f.name),
                                                Ok(None) => state.last_picked_image = Some("Cancelled".into()),
                                                Err(e) => state.last_picked_image = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Camera", PEACH, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        let opts = gpui_mobile::packages::image_picker::ImagePickerOptions {
                                            source: gpui_mobile::packages::image_picker::ImageSource::Camera,
                                            ..Default::default()
                                        };
                                        gpui_mobile::packages::image_picker::pick_image(&opts)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(f)) => state.last_picked_image = Some(f.name),
                                                Ok(None) => state.last_picked_image = Some("Cancelled".into()),
                                                Err(e) => state.last_picked_image = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Multi", YELLOW, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        gpui_mobile::packages::image_picker::pick_multi_image(None, None, None)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(files) => state.last_picked_image = Some(format!("{} images", files.len())),
                                                Err(e) => state.last_picked_image = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Video", TEAL, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        gpui_mobile::packages::image_picker::pick_video(
                                            gpui_mobile::packages::image_picker::ImageSource::Gallery,
                                            gpui_mobile::packages::image_picker::CameraDevice::Rear,
                                        )
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(f)) => state.last_picked_image = Some(f.name),
                                                Ok(None) => state.last_picked_image = Some("Cancelled".into()),
                                                Err(e) => state.last_picked_image = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Record", super::RED, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = run_picker(cx.background_executor(), || {
                                        gpui_mobile::packages::image_picker::pick_video(
                                            gpui_mobile::packages::image_picker::ImageSource::Camera,
                                            gpui_mobile::packages::image_picker::CameraDevice::Rear,
                                        )
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(f)) => state.last_picked_image = Some(f.name),
                                                Ok(None) => state.last_picked_image = Some("Cancelled".into()),
                                                Err(e) => state.last_picked_image = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        ),
                )
        });

    // ── Camera ───────────────────────────────────────────────────────────────
    root = root
        .child(section_header("Camera", sub_text))
        .child({
            let (camera_status, recording, has_handle) = PACKAGES_STATE.with(|s| {
                let state = s.borrow();
                (
                    state.camera_status.as_deref().unwrap_or("Idle").to_string(),
                    state.camera_recording,
                    state.camera_handle.is_some(),
                )
            });

            let cameras_label = PACKAGES_STATE.with(|s| {
                s.borrow().camera_list.clone().unwrap_or_else(|| "Loaded when opened".into())
            });

            let mut card = info_card(card_bg)
                .child(kv_row("Cameras", &cameras_label, PEACH, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(kv_row("Status", &camera_status, PEACH, text_color, sub_text))
                .child(divider_line(divider_color));

            // Row 1: Open / Close / Switch
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .p_3()
                    .child(
                        action_button(
                            if has_handle { "Preview" } else { "Open" },
                            if has_handle { GREEN } else { BLUE },
                            cx.listener(move |_this, _, _, cx| {
                                let preview = PACKAGES_STATE.with(|s| {
                                    let state = s.borrow();
                                    state.camera_handle.map(|id| (id, state.camera_previewing))
                                });
                                match preview {
                                    Some((id, previewing)) if previewing => {
                                        spawn_camera_operation(
                                            cx,
                                            "Stopping preview...",
                                            move || {
                                                let mut handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                                                let result = gpui_mobile::packages::camera::stop_preview(&mut handle);
                                                std::mem::forget(handle);
                                                result
                                            },
                                            move |state, result| {
                                                if state.camera_handle == Some(id) {
                                                    match result {
                                                        Ok(()) => {
                                                            if let Some(view) = state.camera_preview_handle.take() {
                                                                view.dispose();
                                                            }
                                                            state.camera_previewing = false;
                                                            state.camera_status = Some("Preview stopped".into());
                                                        }
                                                        Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    Some((id, _)) => {
                                        spawn_camera_operation(
                                            cx,
                                            "Starting preview...",
                                            move || {
                                                let mut handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                                                gpui_mobile::packages::camera::start_preview(&mut handle)?;
                                                Ok(gpui_mobile::packages::camera::preview_platform_view_handle(&handle))
                                            },
                                            move |state, result| {
                                                if state.camera_handle == Some(id) {
                                                    match result {
                                                        Ok(view) => {
                                                            state.camera_preview_handle = view;
                                                            state.camera_previewing = true;
                                                            state.camera_status = Some("Preview active".into());
                                                        }
                                                        Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    None => {
                                        spawn_camera_operation(
                                            cx,
                                            "Opening camera...",
                                            || {
                                                let cams = gpui_mobile::packages::camera::available_cameras()?;
                                                let names = cams.iter().map(|c| format!("{:?} ({})", c.lens_direction, c.name)).collect::<Vec<_>>().join(", ");
                                                let cam = cams
                                                    .iter()
                                                    .find(|c| c.lens_direction == gpui_mobile::packages::camera::CameraLensDirection::Back)
                                                    .or(cams.first())
                                                    .ok_or_else(|| "No cameras available".to_string())?;
                                                let name = cam.name.clone();
                                                let direction = cam.lens_direction;
                                                let handle = gpui_mobile::packages::camera::create_camera(
                                                    cam,
                                                    gpui_mobile::packages::camera::ResolutionPreset::High,
                                                    true,
                                                )?;
                                                Ok((PendingCamera(Some(handle)), name, direction, names))
                                            },
                                            |state, result| {
                                                match result {
                                                    Ok((mut pending, name, direction, names)) if state.camera_handle.is_none() => {
                                                        let handle = pending.0.take().unwrap();
                                                        let id = handle.id;
                                                        state.camera_handle = Some(id);
                                                        state.camera_direction = Some(direction);
                                                        state.camera_list = Some(names);
                                                        state.camera_status = Some(format!("Opened: {name}"));
                                                    }
                                                    Ok(_) => {}
                                                    Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                                }
                                            },
                                        );
                                    }
                                }
                            }),
                        ),
                    )
                    .child(
                        action_button("Switch", YELLOW, cx.listener(|_this, _, _, cx| {
                            let switch = PACKAGES_STATE.with(|s| {
                                let state = s.borrow();
                                state.camera_handle.map(|id| {
                                    let target = if state.camera_direction == Some(gpui_mobile::packages::camera::CameraLensDirection::Front) {
                                        gpui_mobile::packages::camera::CameraLensDirection::Back
                                    } else {
                                        gpui_mobile::packages::camera::CameraLensDirection::Front
                                    };
                                    (id, target)
                                })
                            });
                            if let Some((id, target)) = switch {
                                spawn_camera_operation(
                                    cx,
                                    "Switching camera...",
                                    move || {
                                        let cams = gpui_mobile::packages::camera::available_cameras()?;
                                        let cam = cams
                                            .iter()
                                            .find(|c| c.lens_direction == target)
                                            .ok_or_else(|| format!("No {target:?} camera available"))?;
                                        let handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                                        let result = gpui_mobile::packages::camera::set_camera(&handle, cam);
                                        std::mem::forget(handle);
                                        result.map(|()| format!("Switched to {target:?}"))
                                    },
                                    move |state, result| {
                                        if state.camera_handle == Some(id) {
                                            match result {
                                                Ok(status) => {
                                                    state.camera_direction = Some(target);
                                                    state.camera_status = Some(status);
                                                }
                                                Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                            }
                                        }
                                    },
                                );
                            }
                        })),
                    )
                    .child(
                        action_button("Close", super::RED, cx.listener(|_this, _, _, cx| {
                            if let Some(id) = PACKAGES_STATE.with(|s| s.borrow().camera_handle) {
                                spawn_camera_operation(
                                    cx,
                                    "Closing camera...",
                                    move || {
                                        gpui_mobile::packages::camera::dispose(
                                            gpui_mobile::packages::camera::CameraHandle::from_id(id),
                                        )
                                    },
                                    move |state, result| {
                                        if state.camera_handle == Some(id) {
                                            match result {
                                                Ok(()) => {
                                                    if let Some(view) = state.camera_preview_handle.take() { view.dispose(); }
                                                    state.camera_handle = None;
                                                    state.camera_direction = None;
                                                    state.camera_previewing = false;
                                                    state.camera_recording = false;
                                                    state.camera_status = Some("Closed".into());
                                                }
                                                Err(e) if e.contains("cleanup continues in background") || e == "Invalid camera handle" => {
                                                    if let Some(view) = state.camera_preview_handle.take() { view.dispose(); }
                                                    state.camera_handle = None;
                                                    state.camera_direction = None;
                                                    state.camera_previewing = false;
                                                    state.camera_recording = false;
                                                    state.camera_status = Some(format!("Error: {e}"));
                                                }
                                                Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                            }
                                        }
                                    },
                                );
                            }
                        })),
                    ),
            );

            // Row 2: Take Photo / Record / Stop
            card = card.child(divider_line(divider_color)).child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(
                        action_button("Photo", TEAL, cx.listener(|_this, _, _, cx| {
                            if let Some(id) = PACKAGES_STATE.with(|s| s.borrow().camera_handle) {
                                spawn_camera_operation(
                                    cx,
                                    "Taking photo...",
                                    move || {
                                        let handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                                        let result = gpui_mobile::packages::camera::take_picture(&handle);
                                        std::mem::forget(handle);
                                        result
                                    },
                                    move |state, result| {
                                        if state.camera_handle == Some(id) {
                                            match result {
                                                Ok(img) => state.camera_status = Some(format!("Photo: {}x{}", img.width, img.height)),
                                                Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                            }
                                        }
                                    },
                                );
                            }
                        })),
                    )
                    .child(
                        action_button(
                            if recording { "Stop Rec" } else { "Record" },
                            if recording { super::RED } else { MAUVE },
                            cx.listener(move |_this, _, _, cx| {
                                let recording = PACKAGES_STATE.with(|s| {
                                    let state = s.borrow();
                                    state.camera_handle.map(|id| (id, state.camera_recording))
                                });
                                match recording {
                                    Some((id, true)) => {
                                        spawn_camera_operation(
                                            cx,
                                            "Stopping recording...",
                                            move || {
                                                let handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                                                let result = gpui_mobile::packages::camera::stop_video_recording(&handle);
                                                std::mem::forget(handle);
                                                result
                                            },
                                            move |state, result| {
                                                if state.camera_handle == Some(id) {
                                                    match result {
                                                        Ok(video) => {
                                                            state.camera_recording = false;
                                                            state.camera_status = Some(format!("Video: {}", video.path));
                                                        }
                                                        Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    Some((id, false)) => {
                                        spawn_camera_operation(
                                            cx,
                                            "Starting recording...",
                                            move || {
                                                let handle = gpui_mobile::packages::camera::CameraHandle::from_id(id);
                                                let result = gpui_mobile::packages::camera::start_video_recording(&handle);
                                                std::mem::forget(handle);
                                                result
                                            },
                                            move |state, result| {
                                                if state.camera_handle == Some(id) {
                                                    match result {
                                                        Ok(()) => {
                                                            state.camera_recording = true;
                                                            state.camera_status = Some("Recording...".into());
                                                        }
                                                        Err(e) => state.camera_status = Some(format!("Error: {e}")),
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    None => {}
                                }
                            }),
                        ),
                    ),
            );

            // Row 3: Flash controls
            card = card.child(divider_line(divider_color)).child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(action_button("Flash Off", SURFACE0, cx.listener(|_this, _, _, cx| {
                        spawn_camera_flash(cx, gpui_mobile::packages::camera::FlashMode::Off, "Flash: Off");
                    })))
                    .child(action_button("Auto", BLUE, cx.listener(|_this, _, _, cx| {
                        spawn_camera_flash(cx, gpui_mobile::packages::camera::FlashMode::Auto, "Flash: Auto");
                    })))
                    .child(action_button("Torch", YELLOW, cx.listener(|_this, _, _, cx| {
                        spawn_camera_flash(cx, gpui_mobile::packages::camera::FlashMode::Torch, "Flash: Torch");
                    }))),
            );

            card
        });

    // ── Permission Handler ────────────────────────────────────────────────
    root = root
        .child(section_header("Permissions", sub_text))
        .child({
            let perm_status = PACKAGES_STATE.with(|s| {
                s.borrow().perm_status.as_deref().unwrap_or("Tap to check").to_string()
            });
            let mut card = info_card(card_bg)
                .child(kv_row("Status", &perm_status, TEAL, text_color, sub_text))
                .child(divider_line(divider_color));

            // Row 1: Check permissions
            card = card.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .p_3()
                    .child(action_button("Camera", BLUE, cx.listener(|_this, _, _, cx| {
                        PACKAGES_STATE.with(|s| {
                            let mut state = s.borrow_mut();
                            match gpui_mobile::packages::permission_handler::check_permission(
                                gpui_mobile::packages::permission_handler::Permission::Camera,
                            ) {
                                Ok(st) => state.perm_status = Some(format!("Camera: {:?}", st)),
                                Err(e) => state.perm_status = Some(format!("Error: {e}")),
                            }
                        });
                        cx.notify();
                    })))
                    .child(action_button("Location", GREEN, cx.listener(|_this, _, _, cx| {
                        PACKAGES_STATE.with(|s| {
                            let mut state = s.borrow_mut();
                            match gpui_mobile::packages::permission_handler::check_permission(
                                gpui_mobile::packages::permission_handler::Permission::LocationWhenInUse,
                            ) {
                                Ok(st) => state.perm_status = Some(format!("Location: {:?}", st)),
                                Err(e) => state.perm_status = Some(format!("Error: {e}")),
                            }
                        });
                        cx.notify();
                    })))
                    .child(action_button("Photos", MAUVE, cx.listener(|_this, _, _, cx| {
                        PACKAGES_STATE.with(|s| {
                            let mut state = s.borrow_mut();
                            match gpui_mobile::packages::permission_handler::check_permission(
                                gpui_mobile::packages::permission_handler::Permission::Photos,
                            ) {
                                Ok(st) => state.perm_status = Some(format!("Photos: {:?}", st)),
                                Err(e) => state.perm_status = Some(format!("Error: {e}")),
                            }
                        });
                        cx.notify();
                    })))
                    .child(action_button("Notif", YELLOW, cx.listener(|_this, _, _, cx| {
                        PACKAGES_STATE.with(|s| {
                            let mut state = s.borrow_mut();
                            match gpui_mobile::packages::permission_handler::check_permission(
                                gpui_mobile::packages::permission_handler::Permission::Notification,
                            ) {
                                Ok(st) => state.perm_status = Some(format!("Notification: {:?}", st)),
                                Err(e) => state.perm_status = Some(format!("Error: {e}")),
                            }
                        });
                        cx.notify();
                    }))),
            );

            // Row 2: Request permissions + open settings
            card = card.child(divider_line(divider_color)).child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(action_button("Request Cam", PEACH, cx.listener(|_this, _, _, cx| {
                        cx.spawn(async |this, cx| {
                            let result = cx.background_executor().spawn(async {
                                gpui_mobile::packages::permission_handler::request_permission(
                                    gpui_mobile::packages::permission_handler::Permission::Camera,
                                )
                            }).await;
                            let _ = this.update(cx, |_this, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match result {
                                        Ok(st) => state.perm_status = Some(format!("Camera: {:?}", st)),
                                        Err(e) => state.perm_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            });
                        }).detach();
                    })))
                    .child(action_button("Request Mic", TEAL, cx.listener(|_this, _, _, cx| {
                        cx.spawn(async |this, cx| {
                            let result = cx.background_executor().spawn(async {
                                gpui_mobile::packages::permission_handler::request_permission(
                                    gpui_mobile::packages::permission_handler::Permission::Microphone,
                                )
                            }).await;
                            let _ = this.update(cx, |_this, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match result {
                                        Ok(st) => state.perm_status = Some(format!("Mic: {:?}", st)),
                                        Err(e) => state.perm_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            });
                        }).detach();
                    })))
                    .child(action_button("Settings", SURFACE0, cx.listener(|_this, _, _, cx| {
                        PACKAGES_STATE.with(|s| {
                            let mut state = s.borrow_mut();
                            match gpui_mobile::packages::permission_handler::open_app_settings() {
                                Ok(true) => state.perm_status = Some("Settings opened".into()),
                                Ok(false) => state.perm_status = Some("Could not open settings".into()),
                                Err(e) => state.perm_status = Some(format!("Error: {e}")),
                            }
                        });
                        cx.notify();
                    }))),
            );

            card
        });

    // ── Location ──────────────────────────────────────────────────────────────
    root = root
        .child(section_header("Location", sub_text))
        .child({
            let last_loc = PACKAGES_STATE.with(|s| {
                s.borrow().location_status.as_deref().unwrap_or("None").to_string()
            });
            info_card(card_bg)
                .child(kv_row("Result", &last_loc, GREEN, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(
                            action_button("Service?", GREEN, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match gpui_mobile::packages::location::is_location_service_enabled() {
                                        Ok(enabled) => state.location_status = Some(format!("Location enabled: {enabled}")),
                                        Err(e) => state.location_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            })),
                        )
                        .child(
                            action_button("Current", BLUE, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = cx.background_executor().spawn(async {
                                        let settings = gpui_mobile::packages::location::LocationSettings::default();
                                        gpui_mobile::packages::location::get_current_position(&settings)
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(pos) => state.location_status = Some(format!(
                                                    "{:.5}, {:.5} (\u{00b1}{:.0}m)",
                                                    pos.latitude, pos.longitude, pos.accuracy
                                                )),
                                                Err(e) => state.location_status = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        )
                        .child(
                            action_button("Last Known", TEAL, cx.listener(|_this, _, _, cx| {
                                cx.spawn(async |this, cx| {
                                    let result = cx.background_executor().spawn(async {
                                        gpui_mobile::packages::location::get_last_known_position()
                                    }).await;
                                    let _ = this.update(cx, |_this, cx| {
                                        PACKAGES_STATE.with(|s| {
                                            let mut state = s.borrow_mut();
                                            match result {
                                                Ok(Some(pos)) => state.location_status = Some(format!(
                                                    "{:.5}, {:.5}",
                                                    pos.latitude, pos.longitude
                                                )),
                                                Ok(None) => state.location_status = Some("No cached location".into()),
                                                Err(e) => state.location_status = Some(format!("Error: {e}")),
                                            }
                                        });
                                        cx.notify();
                                    });
                                }).detach();
                            })),
                        ),
                )
        });

    // ── Notifications ────────────────────────────────────────────────────────
    root = root
        .child(section_header("Notifications", sub_text))
        .child({
            let last_notif = PACKAGES_STATE.with(|s| {
                s.borrow().notif_status.as_deref().unwrap_or("None").to_string()
            });
            info_card(card_bg)
                .child(kv_row("Status", &last_notif, PEACH, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(
                            action_button("Init", PEACH, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match gpui_mobile::packages::notifications::initialize() {
                                        Ok(()) => state.notif_status = Some("Initialized".into()),
                                        Err(e) => state.notif_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            })),
                        )
                        .child(
                            action_button("Show", MAUVE, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    state.notif_counter += 1;
                                    let counter = state.notif_counter;
                                    let notif = gpui_mobile::packages::notifications::Notification {
                                        id: counter,
                                        title: format!("Test #{}", counter),
                                        body: "Hello from GPUI!".into(),
                                        channel: gpui_mobile::packages::notifications::NotificationChannel::default(),
                                        payload: None,
                                    };
                                    match gpui_mobile::packages::notifications::show(&notif) {
                                        Ok(()) => state.notif_status = Some(format!("Shown #{}", counter)),
                                        Err(e) => state.notif_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            })),
                        )
                        .child(
                            action_button("Cancel All", YELLOW, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match gpui_mobile::packages::notifications::cancel_all() {
                                        Ok(()) => state.notif_status = Some("All cancelled".into()),
                                        Err(e) => state.notif_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            })),
                        ),
                )
        });

    // ── Audio Player ─────────────────────────────────────────────────────────
    root = root
        .child(section_header("Audio Player", sub_text))
        .child({
            let audio_st = PACKAGES_STATE.with(|s| {
                s.borrow().audio_status.as_deref().unwrap_or("No player").to_string()
            });
            info_card(card_bg)
                .child(kv_row("Status", &audio_st, TEAL, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(
                            action_button("Create", TEAL, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match gpui_mobile::packages::audio::AudioPlayer::new() {
                                        Ok(p) => {
                                            state.audio_status = Some("Player created".into());
                                            std::mem::forget(p); // leak for demo simplicity
                                        }
                                        Err(e) => state.audio_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            })),
                        )
                        .child(
                            action_button("Info", BLUE, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    s.borrow_mut().audio_status = Some("Audio API ready".into());
                                });
                                cx.notify();
                            })),
                        ),
                )
        });

    // ── Video Player ─────────────────────────────────────────────────────────
    root = root
        .child(section_header("Video Player", sub_text))
        .child({
            let video_st = PACKAGES_STATE.with(|s| {
                s.borrow().video_status.as_deref().unwrap_or("No player").to_string()
            });
            info_card(card_bg)
                .child(kv_row("Status", &video_st, MAUVE, text_color, sub_text))
                .child(divider_line(divider_color))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .p_3()
                        .child(
                            action_button("Create", MAUVE, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    let mut state = s.borrow_mut();
                                    match gpui_mobile::packages::video_player::VideoPlayer::new() {
                                        Ok(p) => {
                                            state.video_status = Some("Player created".into());
                                            std::mem::forget(p); // leak for demo simplicity
                                        }
                                        Err(e) => state.video_status = Some(format!("Error: {e}")),
                                    }
                                });
                                cx.notify();
                            })),
                        )
                        .child(
                            action_button("Info", GREEN, cx.listener(|_this, _, _, cx| {
                                PACKAGES_STATE.with(|s| {
                                    s.borrow_mut().video_status = Some("Video API ready".into());
                                });
                                cx.notify();
                            })),
                        ),
                )
        });

    root
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn path_or_err(r: &Result<std::path::PathBuf, String>) -> String {
    match r {
        Ok(p) => p.display().to_string(),
        Err(e) => format!("Error: {e}"),
    }
}

fn section_header(title: &str, color: u32) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(rgb(color))
        .px_1()
        .child(title.to_string().to_uppercase())
}

fn info_card(bg: u32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .rounded_xl()
        .bg(rgb(bg))
        .overflow_hidden()
}

fn divider_line(color: u32) -> impl IntoElement {
    div().w_full().h(px(1.0)).bg(rgb(color)).mx_3()
}

fn error_card(msg: &str, bg: u32, text_color: u32) -> impl IntoElement {
    info_card(bg).child(
        div()
            .p_4()
            .text_sm()
            .text_color(rgb(text_color))
            .child(format!("Error: {msg}")),
    )
}

fn kv_row(label: &str, value: &str, accent: u32, text_color: u32, sub_text: u32) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_4()
        .py_3()
        .child(div().size_2().rounded_full().bg(rgb(accent)))
        .child(
            div()
                .text_xs()
                .text_color(rgb(sub_text))
                .min_w(px(80.0))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(rgb(text_color))
                .child(value.to_string()),
        )
}

fn action_button(
    label: &str,
    color: u32,
    handler: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .py_2()
        .rounded_lg()
        .bg(rgb(color))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x1e1e2e))
                .child(label.to_string()),
        )
        .on_mouse_down(gpui::MouseButton::Left, handler)
}

fn haptic_button(
    label: &str,
    color: u32,
    handler: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .py_2()
        .rounded_lg()
        .bg(rgb(color))
        .child(
            div()
                .text_xs()
                .text_color(rgb(0x1e1e2e))
                .child(label.to_string()),
        )
        .on_mouse_down(gpui::MouseButton::Left, handler)
}
