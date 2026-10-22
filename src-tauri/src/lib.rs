use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::Emitter;
use tauri_plugin_sql::{Migration, MigrationKind};

mod auto_detect;

use auto_detect::{
    AutoDetectConfig, AutoDetectStatus, CaptureBackend, DetectorEvent, GameWindowInfo,
    LoadingDetector, MenuTemplateTest, NormalizedRegion, Preview, RunEndPayload, RunStartPayload,
    X11Backend,
};

struct AutoDetectHandle {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<AutoDetectStatus>>,
    thread: Option<JoinHandle<()>>,
}

struct AppState {
    d2_game_running: Mutex<bool>,
    auto_detect: Mutex<Option<AutoDetectHandle>>,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg(target_os = "linux")]
fn should_enable_global_shortcuts() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::env::var("XDG_SESSION_TYPE")
            .map(|session_type| session_type != "wayland")
            .unwrap_or(true)
}

#[cfg(not(target_os = "linux"))]
fn should_enable_global_shortcuts() -> bool {
    true
}

#[tauri::command]
fn auto_detect_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    config: AutoDetectConfig,
) -> Result<(), String> {
    stop_auto_detect(&state)?;

    let backend = X11Backend::new().map_err(|error| error.to_string())?;
    let window = backend
        .find_game_window()
        .map_err(|error| error.to_string())?;

    let stop = Arc::new(AtomicBool::new(false));
    let status = Arc::new(Mutex::new(AutoDetectStatus {
        running: true,
        window: Some(window),
        phase: "unknown".into(),
        ..AutoDetectStatus::default()
    }));

    let thread_stop = Arc::clone(&stop);
    let thread_status = Arc::clone(&status);
    let app_handle = app.clone();
    let thread = thread::Builder::new()
        .name("d2-auto-detect".into())
        .spawn(move || run_auto_detect(app_handle, config, thread_stop, thread_status))
        .map_err(|error| format!("cannot start auto-detect thread: {error}"))?;

    {
        let mut guard = state
            .auto_detect
            .lock()
            .map_err(|_| "auto-detect state is poisoned".to_string())?;
        *guard = Some(AutoDetectHandle {
            stop,
            status,
            thread: Some(thread),
        });
    }

    if let Ok(mut running) = state.d2_game_running.lock() {
        *running = true;
    }

    Ok(())
}

#[tauri::command]
fn auto_detect_stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    stop_auto_detect(&state)?;
    if let Ok(mut running) = state.d2_game_running.lock() {
        *running = false;
    }
    Ok(())
}

#[tauri::command]
fn auto_detect_status(state: tauri::State<'_, AppState>) -> AutoDetectStatus {
    let guard = match state.auto_detect.lock() {
        Ok(guard) => guard,
        Err(_) => return AutoDetectStatus::default(),
    };

    match guard.as_ref() {
        Some(handle) => handle
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default(),
        None => AutoDetectStatus::default(),
    }
}

#[tauri::command]
fn find_game_window() -> Result<GameWindowInfo, String> {
    let backend = X11Backend::new().map_err(|error| error.to_string())?;
    backend
        .find_game_window()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn capture_preview(region: NormalizedRegion) -> Result<Preview, String> {
    let backend = X11Backend::new().map_err(|error| error.to_string())?;
    let window = backend
        .find_game_window()
        .map_err(|error| error.to_string())?;
    let pixels = region.to_pixels(&window);
    let image = backend
        .capture_region(&window, pixels)
        .map_err(|error| error.to_string())?;
    let brightness = image.mean_brightness();
    let image = image.to_png_base64().map_err(|error| error.to_string())?;
    Ok(Preview {
        image,
        brightness,
        window,
    })
}

/// One-shot diagnostic: render the menu anchors from the bundled Exocet font
/// and score them against the captured region. Does not touch run detection.
#[tauri::command]
async fn test_menu_templates(region: NormalizedRegion) -> Result<MenuTemplateTest, String> {
    tauri::async_runtime::spawn_blocking(move || run_menu_template_test(region))
        .await
        .map_err(|error| format!("menu template task failed: {error}"))?
}

fn run_menu_template_test(region: NormalizedRegion) -> Result<MenuTemplateTest, String> {
    let backend = X11Backend::new().map_err(|error| error.to_string())?;
    let window = backend
        .find_game_window()
        .map_err(|error| error.to_string())?;
    let pixels = region.to_pixels(&window);
    let captured = backend
        .capture_region(&window, pixels)
        .map_err(|error| error.to_string())?;

    let gray = image::GrayImage::from_fn(captured.width, captured.height, |x, y| {
        let offset = (y as usize * captured.width as usize + x as usize) * 3;
        let red = f32::from(captured.rgb[offset]);
        let green = f32::from(captured.rgb[offset + 1]);
        let blue = f32::from(captured.rgb[offset + 2]);
        let luma = (0.299 * red + 0.587 * green + 0.114 * blue).round();
        image::Luma([luma.clamp(0.0, 255.0) as u8])
    });

    let results = auto_detect::menu_template::test_menu_templates(&gray);

    let mut rgb = captured.rgb.clone();
    auto_detect::menu_template::draw_debug_rectangles(
        &mut rgb,
        captured.width,
        captured.height,
        &results,
    );
    let preview = auto_detect::capture::ImageData {
        width: captured.width,
        height: captured.height,
        rgb,
    }
    .to_png_base64()
    .map_err(|error| error.to_string())?;

    Ok(MenuTemplateTest {
        window,
        preview,
        results,
    })
}

fn stop_auto_detect(state: &AppState) -> Result<(), String> {
    let mut guard = state
        .auto_detect
        .lock()
        .map_err(|_| "auto-detect state is poisoned".to_string())?;

    if let Some(mut handle) = guard.take() {
        handle.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = handle.thread.take() {
            thread
                .join()
                .map_err(|_| "auto-detect thread panicked".to_string())?;
        }
    }

    Ok(())
}

fn update_status(
    status: &Arc<Mutex<AutoDetectStatus>>,
    update: impl FnOnce(&mut AutoDetectStatus),
) {
    if let Ok(mut guard) = status.lock() {
        update(&mut guard);
    }
}

fn set_status_error(status: &Arc<Mutex<AutoDetectStatus>>, message: String) {
    update_status(status, |current| {
        current.last_error = Some(message);
    });
}

fn run_auto_detect(
    app: tauri::AppHandle,
    config: AutoDetectConfig,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<AutoDetectStatus>>,
) {
    let poll_interval = Duration::from_millis(config.poll_interval_ms.max(50));
    let backend = match X11Backend::new() {
        Ok(backend) => backend,
        Err(error) => {
            set_status_error(&status, error.to_string());
            update_status(&status, |current| current.running = false);
            return;
        }
    };
    let mut window = match backend.find_game_window() {
        Ok(window) => window,
        Err(error) => {
            set_status_error(&status, error.to_string());
            update_status(&status, |current| current.running = false);
            return;
        }
    };
    let mut detector = LoadingDetector::new(config.detector_config());
    let mut error_count = 0u32;
    let mut last_window_refresh = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        if last_window_refresh.elapsed() >= Duration::from_secs(5) {
            if let Ok(found) = backend.find_game_window() {
                window = found;
            }
            last_window_refresh = Instant::now();
        }
        let loading_region = config.loading_region.to_pixels(&window);
        match backend.capture_region(&window, loading_region) {
            Ok(image) => {
                error_count = 0;
                let brightness = image.mean_brightness();
                let (menu_brightness, menu_error) = match config.menu_region {
                    Some(menu_region) => {
                        let menu_pixels = menu_region.to_pixels(&window);
                        match backend.capture_region(&window, menu_pixels) {
                            Ok(menu_image) => (Some(menu_image.mean_brightness()), None),
                            Err(error) => (None, Some(error.to_string())),
                        }
                    }
                    None => (None, None),
                };
                let events =
                    detector.process_with_menu(brightness, menu_brightness, Instant::now());
                let phase = detector.phase().as_str().to_string();

                update_status(&status, |current| {
                    current.running = true;
                    current.window = Some(window.clone());
                    current.last_brightness = Some(brightness);
                    current.last_menu_brightness = menu_brightness;
                    current.phase = phase;
                    current.last_error = menu_error;
                });

                for event in events {
                    match event {
                        DetectorEvent::RunStart => {
                            update_status(&status, |current| {
                                current.last_event = Some("run-start".into());
                            });
                            let _ = app.emit(
                                "auto-detect://run-start",
                                RunStartPayload {
                                    source: "screen".into(),
                                },
                            );
                        }
                        DetectorEvent::RunEnd {
                            duration,
                            too_short,
                        } => {
                            update_status(&status, |current| {
                                current.last_event = Some("run-end".into());
                            });
                            let _ = app.emit(
                                "auto-detect://run-end",
                                RunEndPayload {
                                    duration_ms: duration.as_millis() as u64,
                                    too_short,
                                    source: "screen".into(),
                                },
                            );
                        }
                    }
                }
            }
            Err(error) => {
                error_count += 1;
                set_status_error(&status, error.to_string());
                if error_count >= 20 {
                    break;
                }
                if let Ok(found) = backend.find_game_window() {
                    window = found;
                }
            }
        }

        let mut slept = Duration::ZERO;
        while slept < poll_interval && !stop.load(Ordering::Relaxed) {
            let step = Duration::from_millis(25).min(poll_interval - slept);
            thread::sleep(step);
            slept += step;
        }
    }

    update_status(&status, |current| {
        current.running = false;
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let migrations = vec![Migration {
        version: 1,
        description: "create initial tables",
        sql: include_str!("../migrations/init.sql"),
        kind: MigrationKind::Up,
    }];

    let mut builder = tauri::Builder::default();

    if should_enable_global_shortcuts() {
        builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
    }

    builder
        .plugin(
            tauri_plugin_sql::Builder::default()
                .add_migrations("sqlite:run_tracker.db", migrations)
                .build(),
        )
        .manage(AppState {
            d2_game_running: Mutex::new(false),
            auto_detect: Mutex::new(None),
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            auto_detect_start,
            auto_detect_stop,
            auto_detect_status,
            find_game_window,
            capture_preview,
            test_menu_templates
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
