use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::Emitter;
use tauri_plugin_sql::{Migration, MigrationKind};

mod auto_detect;

use auto_detect::{
    hud_visible, menu_groups_visible, required_visible, AutoDetectConfig, AutoDetectStatus,
    CaptureBackend, CombinedDetector, DetectorEvent, GameWindowInfo, HudTemplateConfig,
    HudTemplateMatcher, HudTemplateResult, HudTemplateTest, LiveMenuAnchor, LoadingDetector,
    MenuAnchorGroup, MenuTemplateConfig, MenuTemplateMatcher, MenuTemplateTest, NormalizedRegion,
    Preview, RunEndPayload, RunStartPayload, X11Backend,
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

/// One-shot diagnostic: capture each HUD region on the current game window and
/// score the calibrated image template against it. Does not touch run
/// detection.
#[tauri::command]
async fn test_hud_templates(templates: Vec<HudTemplateConfig>) -> Result<HudTemplateTest, String> {
    tauri::async_runtime::spawn_blocking(move || run_hud_template_test(templates))
        .await
        .map_err(|error| format!("HUD template task failed: {error}"))?
}

fn run_hud_template_test(templates: Vec<HudTemplateConfig>) -> Result<HudTemplateTest, String> {
    if templates.is_empty() {
        return Err("no HUD templates were provided".to_string());
    }

    let mut matchers = Vec::with_capacity(templates.len());
    for template in &templates {
        matchers.push(HudTemplateMatcher::new(template)?);
    }

    let backend = X11Backend::new().map_err(|error| error.to_string())?;
    let window = backend
        .find_game_window()
        .map_err(|error| error.to_string())?;

    let mut results = Vec::with_capacity(matchers.len());
    for matcher in &matchers {
        let pixels = matcher.region().to_pixels(&window);
        let captured = backend
            .capture_region(&window, pixels)
            .map_err(|error| format!("{}: {error}", matcher.name()))?;
        results.push(matcher.score(&captured.to_luma8()));
    }

    let hud_visible = hud_visible(&results, required_visible(matchers.len()));

    Ok(HudTemplateTest {
        window,
        results,
        hud_visible,
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

struct MenuFrame {
    visible: bool,
    /// The CHARGEMENT anchor is visible: the combined detector's loading guard.
    loading_visible: bool,
    max_score: f32,
    anchors: Vec<LiveMenuAnchor>,
    error: Option<String>,
}

/// Capture and score every menu anchor for one frame.
fn capture_menu_frame(
    backend: &X11Backend,
    window: &GameWindowInfo,
    templates: &[ActiveMenuTemplate],
) -> MenuFrame {
    let mut anchors = Vec::with_capacity(templates.len());
    let mut capture_errors = Vec::new();
    let mut left_visible = false;
    let mut center_visible = false;
    let mut right_visible = false;
    let mut loading_visible = false;
    let mut captured_any = false;
    let mut max_score = 0.0_f32;

    for template in templates {
        let menu_pixels = template.config.region.to_pixels(window);
        match backend.capture_region(window, menu_pixels) {
            Ok(menu_image) => {
                captured_any = true;
                let score = template.matcher.score(&menu_image.to_luma8());
                let visible = score >= template.config.threshold;
                max_score = max_score.max(score);
                match template.group {
                    MenuAnchorGroup::Left => left_visible |= visible,
                    MenuAnchorGroup::Center => center_visible |= visible,
                    MenuAnchorGroup::Right => right_visible |= visible,
                    MenuAnchorGroup::Loading => loading_visible |= visible,
                }
                anchors.push(LiveMenuAnchor {
                    text: template.config.text.clone(),
                    group: template.group.as_str().to_string(),
                    score,
                    visible,
                });
            }
            Err(error) => {
                // Skip this anchor for the frame; the other groups may still
                // be captured.
                capture_errors.push(format!("{}: {error}", template.config.text));
            }
        }
    }

    if !captured_any {
        return MenuFrame {
            visible: false,
            loading_visible: false,
            max_score: 0.0,
            anchors,
            error: Some(format!(
                "no menu template region could be captured ({})",
                capture_errors.join("; ")
            )),
        };
    }

    MenuFrame {
        visible: menu_groups_visible(left_visible, center_visible, right_visible),
        loading_visible,
        max_score,
        anchors,
        error: if capture_errors.is_empty() {
            None
        } else {
            Some(format!(
                "menu template capture failed for {}",
                capture_errors.join("; ")
            ))
        },
    }
}

struct HudFrame {
    results: Vec<HudTemplateResult>,
    visible: bool,
    error: Option<String>,
}

/// Capture and score every HUD marker for one frame.
fn capture_hud_frame(
    backend: &X11Backend,
    window: &GameWindowInfo,
    templates: &[HudTemplateMatcher],
) -> HudFrame {
    let mut results = Vec::with_capacity(templates.len());
    let mut capture_errors = Vec::new();

    for template in templates {
        let pixels = template.region().to_pixels(window);
        match backend.capture_region(window, pixels) {
            Ok(image) => results.push(template.score(&image.to_luma8())),
            Err(error) => capture_errors.push(format!("{}: {error}", template.name())),
        }
    }

    let required = required_visible(templates.len());
    HudFrame {
        visible: hud_visible(&results, required),
        results,
        error: if capture_errors.is_empty() {
            None
        } else {
            Some(format!(
                "HUD template capture failed for {}",
                capture_errors.join("; ")
            ))
        },
    }
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
    let mut loading_detector = LoadingDetector::new(config.detector_config());
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

    let mut hud_templates: Vec<HudTemplateMatcher> = Vec::new();
    for template in &config.hud_templates {
        match HudTemplateMatcher::new(template) {
            Ok(matcher) => hud_templates.push(matcher),
            Err(error) => {
                let message = format!(
                    "HUD template '{}' is invalid ({error}); skipping it",
                    template.name
                );
                set_status_error(&status, message.clone());
                template_config_error.get_or_insert(message);
            }
        }
    }

    let has_menu_templates = !menu_templates.is_empty();
    // The menu rule needs one anchor in each of left, center and right. The
    // loading anchor is optional for the rule but feeds the loading guard.
    let menu_ready = [
        MenuAnchorGroup::Left,
        MenuAnchorGroup::Center,
        MenuAnchorGroup::Right,
    ]
    .iter()
    .all(|group| {
        menu_templates
            .iter()
            .any(|template| template.group == *group)
    });
    let hud_ready = !hud_templates.is_empty();
    // The combined detector runs whenever HUD templates are configured; it uses
    // the menu signal when the menu is calibrated, and the HUD alone otherwise.
    let use_combined = hud_ready;
    let mut combined_config = config.combined_detector_config();
    combined_config.hud_missing_ends_run = !menu_ready;
    let mut combined_detector = CombinedDetector::new(combined_config);
    let event_source = if use_combined { "combined" } else { "screen" };
    // In the HUD-only fallback, capturing the menu frame still provides the
    // loading anchor when it is calibrated.
    let capture_menu = menu_ready || (use_combined && has_menu_templates);
    let has_loading_anchor = menu_templates
        .iter()
        .any(|template| template.group == MenuAnchorGroup::Loading);

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
                let menu_frame = if capture_menu {
                    Some(capture_menu_frame(&backend, &window, &menu_templates))
                } else {
                    None
                };
                let hud_frame = if hud_ready {
                    Some(capture_hud_frame(&backend, &window, &hud_templates))
                } else {
                    None
                };

                let mut status_menu_brightness: Option<f32> = None;
                let mut last_menu_score: Option<f32> = None;
                let mut last_menu_anchors: Vec<LiveMenuAnchor> = Vec::new();
                let mut probe_error: Option<String> = None;
                let events;
                let phase;

                if use_combined {
                    let menu_visible = menu_frame.as_ref().is_some_and(|frame| frame.visible);
                    let hud_is_visible = hud_frame.as_ref().is_some_and(|frame| frame.visible);
                    let loading_visible = menu_frame
                        .as_ref()
                        .is_some_and(|frame| frame.loading_visible);
                    events = combined_detector.process(
                        menu_visible,
                        hud_is_visible,
                        loading_visible,
                        Instant::now(),
                    );
                    phase = combined_detector.phase().as_str().to_string();
                    if let Some(frame) = &menu_frame {
                        last_menu_score = Some(frame.max_score);
                        last_menu_anchors = frame.anchors.clone();
                        probe_error = frame.error.clone();
                    }
                } else if menu_ready {
                    let frame = menu_frame
                        .as_ref()
                        .expect("a menu frame is captured when the menu is ready");
                    let menu_signal = if frame.visible { 100.0 } else { 0.0 };
                    events = loading_detector.process_with_menu(
                        brightness,
                        Some(menu_signal),
                        Instant::now(),
                    );
                    phase = loading_detector.phase().as_str().to_string();
                    last_menu_score = Some(frame.max_score);
                    last_menu_anchors = frame.anchors.clone();
                    probe_error = frame.error.clone();
                } else {
                    let (menu_brightness, error) =
                        menu_brightness_probe(&backend, &window, config.menu_region);
                    status_menu_brightness = menu_brightness;
                    probe_error = error;
                    events = loading_detector.process_with_menu(
                        brightness,
                        menu_brightness,
                        Instant::now(),
                    );
                    phase = loading_detector.phase().as_str().to_string();
                }

                let hud_error = hud_frame.as_ref().and_then(|frame| frame.error.clone());
                let last_hud_visible = hud_frame.as_ref().map(|frame| frame.visible);
                let last_loading_visible = if has_loading_anchor {
                    menu_frame.as_ref().map(|frame| frame.loading_visible)
                } else {
                    None
                };
                let last_hud_scores = hud_frame.map(|frame| frame.results).unwrap_or_default();
                let mut menu_error = probe_error.or_else(|| template_config_error.clone());
                if let Some(error) = hud_error {
                    menu_error = Some(match menu_error {
                        Some(existing) => format!("{existing}; {error}"),
                        None => error,
                    });
                }

                update_status(&status, |current| {
                    current.running = true;
                    current.window = Some(window.clone());
                    current.last_brightness = Some(brightness);
                    current.last_menu_brightness = status_menu_brightness;
                    current.last_menu_score = last_menu_score;
                    current.last_menu_anchors = last_menu_anchors;
                    current.last_hud_visible = last_hud_visible;
                    current.last_hud_scores = last_hud_scores;
                    current.last_loading_visible = last_loading_visible;
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
                                    source: event_source.into(),
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
                                    source: event_source.into(),
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
            test_menu_templates,
            test_hud_templates
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
