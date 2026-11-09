//! Opt-in checks that use only the test app's storage and do not request permissions.

pub fn run() {
    use gpui_mobile::packages::{
        battery, connectivity, device_info, package_info, path_provider, permission_handler,
        shared_preferences::SharedPreferences,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut results = Vec::new();
    fn record(results: &mut Vec<String>, name: &str, passed: bool) {
        results.push(format!("{name}: {}", if passed { "PASS" } else { "FAIL" }));
    }
    record(
        &mut results,
        "package_info",
        package_info::get_package_info()
            .is_ok_and(|info| !info.package_name.is_empty() && !info.version.is_empty()),
    );
    record(
        &mut results,
        "device_info",
        device_info::get_device_info()
            .is_ok_and(|info| !info.model.is_empty() && !info.os_version.is_empty()),
    );
    let level = battery::battery_level();
    if level == -1 {
        results.push("battery: SKIP (OS reports unavailable)".into());
    } else {
        record(&mut results, "battery", (0..=100).contains(&level));
    }
    let _ = connectivity::check_connectivity();
    results.push("connectivity: OBSERVED (no independent network assertion)".into());
    record(
        &mut results,
        "camera_permission_query",
        permission_handler::check_permission(permission_handler::Permission::Camera).is_ok(),
    );
    record(
        &mut results,
        "cache_directory",
        path_provider::cache_directory().is_ok_and(|path| path.is_dir()),
    );
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_nanos())
        .unwrap_or(0);
    let key = format!("gpui_platform_smoke_{}_{}", std::process::id(), unique);
    let prefs = SharedPreferences::instance();
    let ok = prefs.set_string(&key, "fixture").is_ok()
        && prefs.get_string(&key).as_deref() == Some("fixture");
    let removed = prefs.remove(&key).is_ok() && !prefs.contains_key(&key);
    record(&mut results, "preferences_round_trip", ok && removed);
    if let Ok(cache) = path_provider::cache_directory() {
        let _ = std::fs::write(cache.join("gpui-platform-smoke.txt"), results.join("\n"));
    }
}

#[cfg(feature = "platform-extended")]
pub fn run_extended() {
    std::thread::spawn(|| {
        use gpui_mobile::packages::{
            audio::AudioPlayer, camera, location, path_provider, permission_handler, sensors,
            video_player::VideoPlayer,
        };
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        std::thread::sleep(Duration::from_secs(1));
        let Ok(cache) = path_provider::cache_directory() else {
            return;
        };
        let _ = std::fs::write(cache.join("gpui-platform-extended.txt"), "RUNNING");
        let mut results = Vec::new();
        let availability = sensors::available_sensors();
        let _ = sensors::accelerometer();
        let _ = sensors::gyroscope();
        let _ = sensors::magnetometer();
        let _ = sensors::barometer();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let (mut accel, mut gyro, mut magnet, mut pressure) = (false, false, false, false);
        while std::time::Instant::now() < deadline {
            let finite =
                |v: sensors::SensorData| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
            accel |= sensors::accelerometer().is_some_and(finite);
            gyro |= sensors::gyroscope().is_some_and(finite);
            magnet |= sensors::magnetometer().is_some_and(finite);
            pressure |=
                sensors::barometer().is_some_and(|v| v.pressure.is_finite() && v.pressure > 0.0);
            if (!availability.accelerometer || accel)
                && (!availability.gyroscope || gyro)
                && (!availability.magnetometer || magnet)
                && (!availability.barometer || pressure)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for (name, available, received) in [
            ("accelerometer", availability.accelerometer, accel),
            ("gyroscope", availability.gyroscope, gyro),
            ("magnetometer", availability.magnetometer, magnet),
            ("barometer", availability.barometer, pressure),
        ] {
            results.push(format!(
                "{name}: {}",
                if !available {
                    "SKIP (unavailable)"
                } else if received {
                    "PASS"
                } else {
                    "FAIL (no valid sample)"
                }
            ));
        }
        results.push(format!(
            "location_service_query: {}",
            if location::is_location_service_enabled().is_ok() {
                "PASS"
            } else {
                "FAIL"
            }
        ));
        results.push(format!(
            "camera_enumeration: {}",
            match camera::available_cameras() {
                Ok(cameras) if !cameras.is_empty() => "PASS",
                Ok(_) => "SKIP (no camera)",
                Err(_) => "FAIL",
            }
        ));
        let granted = |permission| {
            permission_handler::check_permission(permission).is_ok_and(|s| s.is_granted())
        };
        if granted(permission_handler::Permission::Camera) {
            let result = camera::available_cameras().and_then(|cameras| {
                let camera = cameras.first().ok_or("no camera")?;
                let handle = camera::create_camera(camera, camera::ResolutionPreset::Low, false)?;
                camera::dispose(handle)
            });
            results.push(format!(
                "camera_session: {}",
                if result.is_ok() { "PASS" } else { "FAIL" }
            ));
        } else {
            results.push("camera_session: SKIP (permission not granted)".into());
        }
        if granted(permission_handler::Permission::LocationWhenInUse) {
            let valid = location::get_current_position(&location::LocationSettings::default())
                .is_ok_and(|p| {
                    (-90.0..=90.0).contains(&p.latitude) && (-180.0..=180.0).contains(&p.longitude)
                });
            results.push(format!(
                "location_fix: {}",
                if valid { "PASS" } else { "FAIL" }
            ));
        } else {
            results.push("location_fix: SKIP (permission not granted)".into());
        }

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_nanos())
            .unwrap_or(0);
        let wave_path = cache.join(format!("gpui-smoke-{}-{unique}.wav", std::process::id()));
        let video_path = cache.join(format!("gpui-smoke-{}-{unique}.mp4", std::process::id()));
        let sample_count = 8_000u32 * 3;
        let byte_count = sample_count * 2;
        let mut wave = Vec::new();
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + byte_count).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes());
        wave.extend_from_slice(&8_000u32.to_le_bytes());
        wave.extend_from_slice(&16_000u32.to_le_bytes());
        wave.extend_from_slice(&2u16.to_le_bytes());
        wave.extend_from_slice(&16u16.to_le_bytes());
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&byte_count.to_le_bytes());
        wave.resize(44 + byte_count as usize, 0);
        let audio_ok = (|| -> Result<(bool, u64), &'static str> {
            std::fs::write(&wave_path, &wave).map_err(|_| "write")?;
            let player = AudioPlayer::new().map_err(|_| "create")?;
            player
                .set_file_path(wave_path.to_str().ok_or("path")?)
                .map_err(|_| "load")?;
            player.play().map_err(|_| "play")?;
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut sample = (false, 0);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                sample = (
                    player.is_playing().map_err(|_| "state")?,
                    player.position().map_err(|_| "position")?,
                );
                if sample.0 && sample.1 > 0 {
                    break;
                }
            }
            player.stop().map_err(|_| "stop")?;
            Ok(sample)
        })();
        results.push(match audio_ok {
            Ok((true, position)) if position > 0 => "silent_audio_playback: PASS".into(),
            Ok((playing, position)) => {
                format!("silent_audio_playback: FAIL (playing={playing}, position_ms={position})")
            }
            Err(stage) => format!("silent_audio_playback: FAIL ({stage})"),
        });
        let video_ok = (|| -> Result<(u32, u32, bool, u64), &'static str> {
            std::fs::write(&video_path, include_bytes!("../fixtures/black.mp4"))
                .map_err(|_| "write")?;
            let mut player = VideoPlayer::new().map_err(|_| "create")?;
            let info = player
                .set_file_path(video_path.to_str().ok_or("path")?)
                .map_err(|_| "load")?;
            player
                .show_surface(0.0, 0.0, 64.0, 64.0)
                .map_err(|_| "surface")?;
            player.play().map_err(|_| "play")?;
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let mut sample = (false, 0);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                sample = (
                    player.is_playing().map_err(|_| "state")?,
                    player.position().map_err(|_| "position")?,
                );
                if sample.0 && sample.1 > 0 {
                    break;
                }
            }
            player.pause().map_err(|_| "pause")?;
            Ok((info.width, info.height, sample.0, sample.1))
        })();
        results.push(match video_ok {
            Ok((64, 64, true, position)) if position > 0 => "silent_video_playback: PASS".into(),
            Ok((w, h, playing, position)) => format!("silent_video_playback: FAIL (size={w}x{h}, playing={playing}, position_ms={position})"),
            Err(stage) => format!("silent_video_playback: FAIL ({stage})"),
        });
        let _ = std::fs::remove_file(wave_path);
        let _ = std::fs::remove_file(video_path);
        let _ = std::fs::write(cache.join("gpui-platform-extended.txt"), results.join("\n"));
    });
}

#[cfg(all(feature = "platform-device", target_os = "android"))]
pub fn run_device() {
    std::thread::spawn(|| {
        use gpui_mobile::packages::{
            camera, notifications, path_provider, permission_handler as permissions,
        };
        use gpui_mobile::platform_view::PlatformViewBounds;
        use std::time::Duration;

        std::thread::sleep(Duration::from_secs(2));
        let Ok(cache) = path_provider::cache_directory() else {
            return;
        };
        let report = cache.join("gpui-platform-device.txt");
        let mut results = Vec::<String>::new();
        let record = |results: &mut Vec<String>, line: &str| {
            results.push(line.into());
            let _ = std::fs::write(&report, results.join("\n"));
        };
        record(&mut results, "camera_permission: REQUESTING");
        let camera_granted = permissions::request_permission(permissions::Permission::Camera)
            .is_ok_and(|status| status.is_granted());
        record(
            &mut results,
            if camera_granted {
                "camera_permission: PASS"
            } else {
                "camera_permission: FAIL"
            },
        );
        if camera_granted {
            std::thread::sleep(Duration::from_secs(2));
            let mut switched = false;
            let result = (|| -> Result<(), &'static str> {
                let cameras = camera::available_cameras().map_err(|_| "enumerate")?;
                let description = cameras.first().ok_or("no camera")?;
                let mut handle =
                    camera::create_camera(description, camera::ResolutionPreset::Low, false)
                        .map_err(|_| "create")?;
                let outcome = (|| {
                    camera::start_preview(&mut handle).map_err(|_| "preview")?;
                    let view = camera::preview_platform_view_handle(&handle).ok_or("view")?;
                    view.set_bounds(PlatformViewBounds {
                        x: 0.0,
                        y: 0.0,
                        width: 160.0,
                        height: 160.0,
                    });
                    view.set_visible(true);
                    std::thread::sleep(Duration::from_secs(8));
                    let capture = || -> Result<(), &'static str> {
                        let image = camera::take_picture(&handle).map_err(|_| "capture")?;
                        let path = std::path::Path::new(&image.path);
                        let owned = path.parent() == Some(cache.as_path())
                            && path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .is_some_and(|n| n.starts_with("photo_") && n.ends_with(".jpg"));
                        if !owned {
                            return Err("unexpected_path");
                        }
                        let valid = image.width > 0
                            && image.height > 0
                            && std::fs::read(path).is_ok_and(|bytes| {
                                bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9])
                            });
                        if std::fs::remove_file(path).is_err() {
                            return Err("cleanup");
                        }
                        if valid { Ok(()) } else { Err("invalid_image") }
                    };
                    capture()?;
                    if let Some(other) = cameras
                        .iter()
                        .find(|c| c.lens_direction != description.lens_direction)
                    {
                        camera::set_camera(&handle, other).map_err(|_| "switch")?;
                        capture()?;
                        switched = true;
                    }
                    Ok(())
                })();
                let disposed = camera::dispose(handle).is_ok();
                if !disposed {
                    return Err("dispose");
                }
                outcome
            })();
            record(
                &mut results,
                &match result {
                    Ok(()) => "camera_capture: PASS".into(),
                    Err(stage) => format!("camera_capture: FAIL ({stage})"),
                },
            );
            record(
                &mut results,
                if switched {
                    "camera_switch_capture: PASS"
                } else {
                    "camera_switch_capture: SKIP or FAIL"
                },
            );
        }
        record(&mut results, "notification_permission: REQUESTING");
        let granted = permissions::request_permission(permissions::Permission::Notification)
            .is_ok_and(|status| status.is_granted());
        record(
            &mut results,
            if granted {
                "notification_permission: PASS"
            } else {
                "notification_permission: FAIL"
            },
        );
        if granted {
            let id = 0x47505549;
            let notification = notifications::Notification {
                id,
                title: "GPUI test".into(),
                body: "Temporary test notification".into(),
                channel: notifications::NotificationChannel::default(),
                payload: None,
            };
            if notifications::initialize()
                .and_then(|_| notifications::show(&notification))
                .is_ok()
            {
                record(&mut results, "notification: POSTED");
                std::thread::sleep(Duration::from_secs(20));
                record(
                    &mut results,
                    if notifications::cancel(id).is_ok() {
                        "notification_cancel: PASS"
                    } else {
                        "notification_cancel: FAIL"
                    },
                );
            } else {
                record(&mut results, "notification: FAIL (show)");
            }
        }
        record(&mut results, "done");
    });
}
