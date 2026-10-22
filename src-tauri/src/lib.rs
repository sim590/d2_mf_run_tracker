use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::Emitter;
use tauri_plugin_sql::{Migration, MigrationKind};

mod auto_detect;

use auto_detect::{
    menu_groups_visible, AutoDetectConfig, AutoDetectStatus, CaptureBackend, DetectorEvent,
    GameWindowInfo, LiveMenuAnchor, LoadingDetector, MenuAnchorGroup, MenuTemplateConfig,
    MenuTemplateMatcher, MenuTemplateTest, NormalizedRegion, Preview, RunEndPayload,
    RunStartPayload, X11Backend,
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
async fn test_menu_templates(
    region: NormalizedRegion,
    anchor: Option<String>,
) -> Result<MenuTemplateTest, String> {
    tauri::async_runtime::spawn_blocking(move || run_menu_template_test(region, anchor))
        .await
        .map_err(|error| format!("menu template task failed: {error}"))?
}

fn run_menu_template_test(
    region: NormalizedRegion,
    anchor: Option<String>,
) -> Result<MenuTemplateTest, String> {
    let backend = X11Backend::new().map_err(|error| error.to_string())?;
    let window = backend
        .find_game_window()
        .map_err(|error| error.to_string())?;
    let pixels = region.to_pixels(&window);
    let captured = backend
        .capture_region(&window, pixels)
        .map_err(|error| error.to_string())?;

    let gray = captured.to_luma8();
    let results = auto_detect::menu_template::test_menu_templates(&gray, anchor.as_deref());

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

fn menu_brightness_probe(
    backend: &X11Backend,
    window: &GameWindowInfo,
    region: Option<NormalizedRegion>,
) -> (Option<f32>, Option<String>) {
    match region {
        Some(menu_region) => {
            let menu_pixels = menu_region.to_pixels(window);
            match backend.capture_region(window, menu_pixels) {
                Ok(menu_image) => (Some(menu_image.mean_brightness()), None),
                Err(error) => (None, Some(error.to_string())),
            }
        }
        None => (None, None),
    }
}

struct ActiveMenuTemplate {
    config: MenuTemplateConfig,
    matcher: MenuTemplateMatcher,
    group: MenuAnchorGroup,
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

    let mut template_config_error: Option<String> = None;
    let mut menu_templates: Vec<ActiveMenuTemplate> = Vec::new();
    for template in &config.menu_templates {
        let Some(group) = MenuAnchorGroup::parse(&template.group) else {
            let message = format!(
                "menu template '{}' has an invalid group '{}'; skipping it",
                template.text, template.group
            );
            set_status_error(&status, message.clone());
            template_config_error.get_or_insert(message);
            continue;
        };
        match MenuTemplateMatcher::new(
            &template.text,
            &template.font,
            template.size,
            template.letter_spacing,
        ) {
            Some(matcher) => menu_templates.push(ActiveMenuTemplate {
                config: template.clone(),
                matcher,
                group,
            }),
            None => {
                let message = format!("menu template '{}' is invalid; skipping it", template.text);
                set_status_error(&status, message.clone());
                template_config_error.get_or_insert(message);
            }
        }
    }

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
                let (
                    menu_signal,
                    status_menu_brightness,
                    last_menu_score,
                    last_menu_anchors,
                    probe_error,
                ) = if menu_templates.is_empty() {
                    let (brightness, error) =
                        menu_brightness_probe(&backend, &window, config.menu_region);
                    (brightness, brightness, None, Vec::new(), error)
                } else {
                    let mut anchors = Vec::with_capacity(menu_templates.len());
                    let mut capture_errors = Vec::new();
                    let mut left_visible = false;
                    let mut center_visible = false;
                    let mut right_visible = false;
                    let mut captured_any = false;
                    let mut max_score = 0.0_f32;

                    for template in &menu_templates {
                        let menu_pixels = template.config.region.to_pixels(&window);
                        match backend.capture_region(&window, menu_pixels) {
                            Ok(menu_image) => {
                                captured_any = true;
                                let score = template.matcher.score(&menu_image.to_luma8());
                                let visible = score >= template.config.threshold;
                                max_score = max_score.max(score);
                                match template.group {
                                    MenuAnchorGroup::Left => left_visible |= visible,
                                    MenuAnchorGroup::Center => center_visible |= visible,
                                    MenuAnchorGroup::Right => right_visible |= visible,
                                }
                                anchors.push(LiveMenuAnchor {
                                    text: template.config.text.clone(),
                                    group: template.group.as_str().to_string(),
                                    score,
                                    visible,
                                });
                            }
                            Err(error) => {
                                // Skip this anchor for the frame; the other
                                // groups may still be captured.
                                capture_errors.push(format!("{}: {error}", template.config.text));
                            }
                        }
                    }

                    if captured_any {
                        let menu_visible =
                            menu_groups_visible(left_visible, center_visible, right_visible);
                        let error = if capture_errors.is_empty() {
                            None
                        } else {
                            Some(format!(
                                "menu template capture failed for {}",
                                capture_errors.join("; ")
                            ))
                        };
                        (
                            Some(if menu_visible { 100.0 } else { 0.0 }),
                            None,
                            Some(max_score),
                            anchors,
                            error,
                        )
                    } else {
                        let (brightness, fallback_error) =
                            menu_brightness_probe(&backend, &window, config.menu_region);
                        let mut message = format!(
                            "no menu template region could be captured ({})",
                            capture_errors.join("; ")
                        );
                        if let Some(fallback) = fallback_error {
                            message.push_str(&format!("; brightness fallback failed: {fallback}"));
                        } else {
                            message.push_str("; using brightness fallback");
                        }
                        (brightness, brightness, None, Vec::new(), Some(message))
                    }
                };
                let menu_error = probe_error.or_else(|| template_config_error.clone());
                let events = detector.process_with_menu(brightness, menu_signal, Instant::now());
                let phase = detector.phase().as_str().to_string();

                update_status(&status, |current| {
                    current.running = true;
                    current.window = Some(window.clone());
                    current.last_brightness = Some(brightness);
                    current.last_menu_brightness = status_menu_brightness;
                    current.last_menu_score = last_menu_score;
                    current.last_menu_anchors = last_menu_anchors;
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
