#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use pulso::estimate::{DeviceView, Tracker};
use pulso::model::Charge;
use pulso::monitor::{self, Command};
use pulso::sys;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow, WindowEvent};

const WIDTH: f64 = 348.0;
const MARGIN: f64 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Flyout,
    Floating,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    mode: Mode,
    tray_icons: bool,
    autostart: bool,
    on_top: bool,
    pos: Option<(i32, i32)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: Mode::Flyout, tray_icons: true, autostart: false, on_top: true, pos: None }
    }
}

struct State {
    tracker: Tracker,
    views: Vec<DeviceView>,
    settings: Settings,
    last_save: i64,
    hidden_at: Instant,
    /// Windows may report a focus loss right after showing (focus-steal guard); ignore that blip.
    shown_at: Instant,
    /// A show was requested while the webview was still loading.
    pending_show: bool,
    /// Last tray icon rect (physical x, y, w, h) used to anchor the flyout.
    anchor: Option<(f64, f64, f64, f64)>,
    /// Tray ids currently shown, and the last glyph/tooltip pushed to each (skip redundant updates).
    trays: Vec<(String, String)>,
    alerts: pulso::estimate::Alerts,
}

struct Ctx {
    state: Mutex<State>,
    cmds: Mutex<Sender<Command>>,
    menu: Menu<tauri::Wry>,
    m_floating: CheckMenuItem<tauri::Wry>,
    m_tray: CheckMenuItem<tauri::Wry>,
    m_autostart: CheckMenuItem<tauri::Wry>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Pulso")
}

fn load<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    std::fs::read(data_dir().join(name)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Atomic write (temp + rename) so a crash mid-write never corrupts history.
fn save<T: Serialize>(name: &str, v: &T) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let tmp = dir.join(format!("{name}.tmp"));
    if let Ok(b) = serde_json::to_vec(v) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(name));
        }
    }
}

// ---------- tray ----------

fn tooltip(v: &DeviceView) -> String {
    let pct = v.percent.map_or("—".into(), |p| format!("{p}%"));
    let detail = if !v.online {
        "em repouso".to_string()
    } else {
        match v.charge {
            Some(Charge::Charging) => v.to_full.map_or("carregando".into(), |s| format!("carregando · cheio em {}", pulso::estimate::human(s))),
            Some(Charge::Full) => "carregado".into(),
            _ => v.remaining.map_or(String::new(), |s| format!("~{} de uso", pulso::estimate::human(s))),
        }
    };
    format!("{} · {pct} · {detail}", v.name)
}

fn glyph(v: &DeviceView, light: bool, size: u32) -> Image<'static> {
    let text = v.percent.map_or("–".to_string(), |p| p.to_string());
    let base = if light { [0x1C, 0x1C, 0x1E] } else { [0xFF, 0xFF, 0xFF] };
    let rgb = match (v.charge, v.percent) {
        (Some(Charge::Charging), _) => if light { [0x1A, 0x9E, 0x4B] } else { [0x4A, 0xE0, 0x82] },
        (_, Some(p)) if p <= 15 => if light { [0xD7, 0x26, 0x1E] } else { [0xFF, 0x5A, 0x4F] },
        _ => base,
    };
    let alpha = if v.online { 1.0 } else { 0.42 };
    Image::new_owned(sys::render_glyph(&text, rgb, alpha, size), size, size)
}

fn sync_trays(app: &AppHandle, ctx: &Ctx, st: &mut State) {
    let light = sys::taskbar_is_light();
    let size = sys::tray_size();
    let mut want: Vec<(String, Option<&DeviceView>)> = if st.settings.tray_icons && !st.views.is_empty() {
        st.views.iter().map(|v| (format!("d:{}", v.id), Some(v))).collect()
    } else {
        vec![("main".into(), None)]
    };
    // Windows lays tray icons out right-to-left in creation order; reverse so the panel order reads left-to-right.
    want.reverse();

    for (id, _) in st.trays.clone() {
        if !want.iter().any(|(w, _)| *w == id) {
            let _ = app.remove_tray_by_id(&id);
            st.trays.retain(|(t, _)| *t != id);
        }
    }
    let mut created = false;
    for (id, view) in want {
        let (icon, tip, key) = match view {
            Some(v) => {
                let key = format!("{:?}{:?}{}{}{}", v.percent, v.charge, v.online, light, size);
                (glyph(v, light, size), tooltip(v), key)
            }
            None => {
                let icon = app.default_window_icon().cloned().map(|i| i.to_owned()).unwrap_or_else(|| Image::new_owned(vec![0; 4], 1, 1));
                (icon, "Pulso".to_string(), "main".to_string())
            }
        };
        if let Some(tray) = app.tray_by_id(&id) {
            let prev = st.trays.iter().find(|(t, _)| *t == id).map(|(_, k)| k.clone());
            if prev.as_deref() != Some(&key) {
                let _ = tray.set_icon(Some(icon));
                st.trays.iter_mut().filter(|(t, _)| *t == id).for_each(|(_, k)| *k = key.clone());
            }
            let _ = tray.set_tooltip(Some(&tip));
            continue;
        }
        let built = TrayIconBuilder::with_id(&id)
            .icon(icon)
            .tooltip(&tip)
            .menu(&ctx.menu)
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, ev| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = ev {
                    let p = rect.position.to_physical::<f64>(1.0);
                    let s = rect.size.to_physical::<f64>(1.0);
                    toggle_panel(tray.app_handle(), Some((p.x, p.y, s.width, s.height)));
                }
            })
            .build(app);
        if built.is_ok() {
            st.trays.push((id, key));
            created = true;
        }
    }
    if created {
        // Explorer writes the NotifyIconSettings entry a moment after the icon appears.
        std::thread::spawn(|| {
            for _ in 0..3 {
                std::thread::sleep(Duration::from_secs(2));
                sys::promote_tray_icons();
            }
        });
    }
}

// ---------- panel window ----------

fn panel(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("panel")
}

#[derive(Clone, Copy)]
struct Placement {
    mode: Mode,
    pos: Option<(i32, i32)>,
    anchor: Option<(f64, f64, f64, f64)>,
}

impl State {
    fn placement(&self) -> Placement {
        Placement { mode: self.settings.mode, pos: self.settings.pos, anchor: self.anchor }
    }
}

/// Places the flyout above the tray icon that opened it, inside the monitor's work area.
/// Never call with the state lock held: moving the window fires `Moved`, whose handler locks it.
fn place(app: &AppHandle, win: &WebviewWindow, p: Placement) {
    let size = win.outer_size().unwrap_or_default();
    let (w, h) = (size.width as f64, size.height as f64);
    if p.mode == Mode::Floating {
        if let Some((x, y)) = p.pos {
            // Keep it reachable if the monitor layout changed.
            if app.monitor_from_point(x as f64 + 20.0, y as f64 + 20.0).ok().flatten().is_some() {
                let _ = win.set_position(PhysicalPosition::new(x, y));
                return;
            }
        }
    }
    let (ax, ay, aw, ah) = p.anchor.unwrap_or_else(|| {
        let m = win.primary_monitor().ok().flatten();
        let (mw, mh) = m.map_or((1920.0, 1080.0), |m| (m.size().width as f64, m.size().height as f64));
        (mw - 120.0, mh - 24.0, 24.0, 24.0)
    });
    let Some(mon) = app.monitor_from_point(ax + aw / 2.0, ay + ah / 2.0).ok().flatten().or_else(|| win.primary_monitor().ok().flatten()) else {
        return;
    };
    let scale = mon.scale_factor();
    let wa = mon.work_area();
    let (wx, wy) = (wa.position.x as f64, wa.position.y as f64);
    let (ww, wh) = (wa.size.width as f64, wa.size.height as f64);
    let m = MARGIN * scale;
    let x = (ax + aw / 2.0 - w / 2.0).clamp(wx + m, wx + ww - w - m);
    // Taskbar at the top: drop down; otherwise rise from the bottom edge. An auto-hide taskbar
    // reports a full-screen work area, so never overlap the tray icon that was clicked.
    let bottom = if p.anchor.is_some() { (wy + wh).min(ay) } else { wy + wh };
    let y = if ay < wy { wy + m } else { bottom - h - m };
    let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

/// Hidden flyout: suspend the renderer and ask WebView2 to trim memory; resume before showing.
fn set_webview_idle(win: &WebviewWindow, idle: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use windows::core::Interface;
    let _ = win.with_webview(move |wv| unsafe {
        let controller = wv.controller();
        let Ok(core) = controller.CoreWebView2() else { return };
        if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
            let level = if idle { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW } else { COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL };
            let _ = c19.SetMemoryUsageTargetLevel(level);
        }
        if let Ok(c3) = core.cast::<ICoreWebView2_3>() {
            if idle {
                // TrySuspend only works on an invisible webview.
                let _ = controller.SetIsVisible(false);
                let handler = webview2_com::TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = c3.TrySuspend(&handler);
            } else {
                let _ = c3.Resume();
                let _ = controller.SetIsVisible(true);
            }
        }
    });
}

/// The panel's webview only exists while it is useful: it is created on first open and destroyed
/// after a few idle minutes hidden, so a resting Pulso is just the small Rust process.
const IDLE_DESTROY: Duration = Duration::from_secs(180);

fn create_panel(app: &AppHandle) -> Option<WebviewWindow> {
    let cfg = app.config().app.windows.iter().find(|w| w.label == "panel")?.clone();
    let win = tauri::WebviewWindowBuilder::from_config(app, &cfg).ok()?.build().ok()?;
    if let Ok(hwnd) = win.hwnd() {
        sys::round_corners(hwnd.0 as isize);
    }
    let h = app.clone();
    win.on_window_event(move |ev| {
        let ctx = h.state::<Arc<Ctx>>();
        match ev {
            WindowEvent::Focused(false) => {
                let (mode, fresh) = {
                    let st = ctx.state.lock().unwrap();
                    (st.settings.mode, st.shown_at.elapsed() < Duration::from_millis(500))
                };
                if mode == Mode::Flyout && !fresh && panel(&h).and_then(|w| w.is_visible().ok()).unwrap_or(false) {
                    let _ = h.emit_to("panel", "dismiss", ());
                }
            }
            WindowEvent::Moved(p) => {
                let visible = panel(&h).and_then(|w| w.is_visible().ok()).unwrap_or(false);
                let mut st = ctx.state.lock().unwrap();
                if st.settings.mode == Mode::Floating && visible {
                    st.settings.pos = Some((p.x, p.y));
                }
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = h.emit_to("panel", "dismiss", ());
            }
            _ => {}
        }
    });
    Some(win)
}

fn show_panel(app: &AppHandle) {
    let ctx = app.state::<Arc<Ctx>>();
    let win = match panel(app) {
        Some(w) => w,
        None => {
            // Fresh webview: show it once the page reports `ready`, never as an empty frame.
            ctx.state.lock().unwrap().pending_show = true;
            create_panel(app);
            return;
        }
    };
    set_webview_idle(&win, false);
    let (p, on_top) = {
        let mut st = ctx.state.lock().unwrap();
        st.shown_at = Instant::now();
        (st.placement(), st.settings.mode == Mode::Flyout || st.settings.on_top)
    };
    place(app, &win, p);
    let _ = win.set_always_on_top(on_top);
    let _ = win.show();
    let _ = win.set_focus();
    let _ = app.emit_to("panel", "shown", ());
    let _ = ctx.cmds.lock().unwrap().send(Command::Refresh);
}

fn toggle_panel(app: &AppHandle, anchor: Option<(f64, f64, f64, f64)>) {
    let ctx = app.state::<Arc<Ctx>>();
    let (mode, recently_hidden) = {
        let mut st = ctx.state.lock().unwrap();
        if anchor.is_some() {
            st.anchor = anchor;
        }
        (st.settings.mode, st.hidden_at.elapsed() < Duration::from_millis(350))
    };
    if panel(app).and_then(|w| w.is_visible().ok()).unwrap_or(false) {
        let Some(win) = panel(app) else { return };
        if mode == Mode::Flyout {
            let _ = app.emit_to("panel", "dismiss", ());
        } else {
            let _ = win.set_focus();
        }
    } else if !recently_hidden {
        show_panel(app);
    }
}

// ---------- commands ----------

#[derive(Serialize)]
struct Snapshot {
    devices: Vec<DeviceView>,
    settings: Settings,
    now: i64,
}

#[tauri::command]
fn snapshot(ctx: tauri::State<Arc<Ctx>>) -> Snapshot {
    let st = ctx.state.lock().unwrap();
    Snapshot { devices: st.views.clone(), settings: st.settings.clone(), now: now() }
}

#[tauri::command]
fn fit(app: AppHandle, ctx: tauri::State<Arc<Ctx>>, height: f64) {
    let Some(win) = panel(&app) else { return };
    let cur = win.inner_size().unwrap_or_default().height as f64 / win.scale_factor().unwrap_or(1.0);
    if (cur - height).abs() < 1.0 {
        return;
    }
    let _ = win.set_size(LogicalSize::new(WIDTH, height.clamp(120.0, 900.0)));
    let p = ctx.state.lock().unwrap().placement();
    if p.mode == Mode::Flyout {
        place(&app, &win, p);
    }
}

#[tauri::command]
fn hide(app: AppHandle, ctx: tauri::State<Arc<Ctx>>) {
    if let Some(win) = panel(&app) {
        let _ = win.hide();
        set_webview_idle(&win, true);
    }
    let stamp = Instant::now();
    ctx.state.lock().unwrap().hidden_at = stamp;
    std::thread::spawn(move || {
        let idle = std::env::var("PULSO_IDLE_SECS").ok().and_then(|s| s.parse().ok()).map_or(IDLE_DESTROY, Duration::from_secs);
        std::thread::sleep(idle);
        let h = app.clone();
        let _ = app.run_on_main_thread(move || {
            let ctx = h.state::<Arc<Ctx>>();
            let st = ctx.state.lock().unwrap();
            let untouched = st.hidden_at == stamp && st.settings.mode == Mode::Flyout && !st.pending_show;
            drop(st);
            if let Some(win) = panel(&h) {
                if untouched && !win.is_visible().unwrap_or(true) {
                    let _ = win.destroy();
                }
            }
        });
    });
}

/// The page finished loading; reveal a panel that was waiting for its first paint.
#[tauri::command]
fn ready(app: AppHandle, ctx: tauri::State<Arc<Ctx>>) {
    let pending = std::mem::take(&mut ctx.state.lock().unwrap().pending_show);
    if pending {
        show_panel(&app);
    }
}

#[tauri::command]
fn refresh(ctx: tauri::State<Arc<Ctx>>) {
    let _ = ctx.cmds.lock().unwrap().send(Command::Refresh);
}

#[tauri::command]
fn update_settings(app: AppHandle, patch: serde_json::Value) {
    apply_settings(&app, patch);
}

#[tauri::command]
fn quit(app: AppHandle) {
    persist(&app);
    app.exit(0);
}

fn apply_settings(app: &AppHandle, patch: serde_json::Value) {
    let ctx = app.state::<Arc<Ctx>>().inner().clone();
    let mut st = ctx.state.lock().unwrap();
    let mut merged = serde_json::to_value(&st.settings).unwrap_or_default();
    if let (Some(m), Some(p)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            m.insert(k.clone(), v.clone());
        }
    }
    let Ok(new) = serde_json::from_value::<Settings>(merged) else { return };
    let old = std::mem::replace(&mut st.settings, new.clone());
    if old.autostart != new.autostart {
        sys::set_autostart(new.autostart);
    }
    let _ = ctx.m_floating.set_checked(new.mode == Mode::Floating);
    let _ = ctx.m_tray.set_checked(new.tray_icons);
    let _ = ctx.m_autostart.set_checked(new.autostart);
    if old.tray_icons != new.tray_icons {
        sync_trays(app, &ctx, &mut st);
    }
    save("settings.json", &st.settings);
    let _ = app.emit_to("panel", "settings", &st.settings);
    drop(st);
    let win = panel(app);
    if let Some(win) = &win {
        let _ = win.set_always_on_top(new.mode == Mode::Flyout || new.on_top);
    }
    if old.mode != new.mode {
        if new.mode == Mode::Floating {
            // Detach in place: an open flyout stays exactly where it is.
            if let Some(p) = win.as_ref().filter(|w| w.is_visible().unwrap_or(false)).and_then(|w| w.outer_position().ok()) {
                ctx.state.lock().unwrap().settings.pos = Some((p.x, p.y));
            }
            show_panel(app);
        } else {
            let _ = app.emit_to("panel", "dismiss", ());
        }
    }
}

fn persist(app: &AppHandle) {
    let ctx = app.state::<Arc<Ctx>>();
    let st = ctx.state.lock().unwrap();
    save("history.json", &st.tracker);
    save("settings.json", &st.settings);
}

// ---------- app ----------

fn main() {
    let settings: Settings = load("settings.json");
    let first_run = !data_dir().join("settings.json").exists();
    if settings.autostart {
        // Re-assert in case the exe moved.
        sys::set_autostart(true);
    }

    tauri::Builder::default()
        // Launching Pulso again (Start menu, shortcut) just opens the running instance's panel.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_panel(app)))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![snapshot, fit, hide, ready, refresh, update_settings, quit])
        .setup(move |app| {
            let handle = app.handle().clone();
            let m_open = MenuItem::with_id(app, "open", "Abrir painel", true, None::<&str>)?;
            let m_floating = CheckMenuItem::with_id(app, "floating", "Painel flutuante", true, settings.mode == Mode::Floating, None::<&str>)?;
            let m_tray = CheckMenuItem::with_id(app, "tray", "Porcentagem na bandeja", true, settings.tray_icons, None::<&str>)?;
            let m_autostart = CheckMenuItem::with_id(app, "autostart", "Iniciar com o Windows", true, settings.autostart, None::<&str>)?;
            let m_refresh = MenuItem::with_id(app, "refresh", "Atualizar agora", true, None::<&str>)?;
            let m_quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
            let menu = MenuBuilder::new(app)
                .items(&[&m_open])
                .separator()
                .items(&[&m_floating, &m_tray, &m_autostart])
                .separator()
                .items(&[&m_refresh, &m_quit])
                .build()?;

            let (tx, rx) = mpsc::channel();
            let mut tracker: Tracker = load("history.json");
            // The app was closed: nothing was observed meanwhile, so no device is known to be online.
            tracker.devices.values_mut().for_each(|t| t.online = false);
            let views = tracker.views(now());
            let ctx = Arc::new(Ctx {
                state: Mutex::new(State {
                    tracker,
                    views,
                    settings: settings.clone(),
                    last_save: now(),
                    hidden_at: Instant::now() - Duration::from_secs(5),
                    shown_at: Instant::now(),
                    pending_show: false,
                    anchor: None,
                    trays: Vec::new(),
                    alerts: Default::default(),
                }),
                cmds: Mutex::new(tx),
                menu,
                m_floating,
                m_tray,
                m_autostart,
            });
            app.manage(ctx.clone());

            app.on_menu_event(|app, ev| match ev.id().as_ref() {
                "open" => show_panel(app),
                "floating" => {
                    let floating = app.state::<Arc<Ctx>>().state.lock().unwrap().settings.mode != Mode::Floating;
                    apply_settings(app, serde_json::json!({ "mode": if floating { "floating" } else { "flyout" } }));
                }
                "tray" => {
                    let on = !app.state::<Arc<Ctx>>().state.lock().unwrap().settings.tray_icons;
                    apply_settings(app, serde_json::json!({ "tray_icons": on }));
                }
                "autostart" => {
                    let on = !app.state::<Arc<Ctx>>().state.lock().unwrap().settings.autostart;
                    apply_settings(app, serde_json::json!({ "autostart": on }));
                }
                "refresh" => {
                    let _ = app.state::<Arc<Ctx>>().cmds.lock().unwrap().send(Command::Refresh);
                }
                "quit" => {
                    persist(app);
                    app.exit(0);
                }
                _ => {}
            });

            {
                let mut st = ctx.state.lock().unwrap();
                sync_trays(&handle, &ctx, &mut st);
            }

            // Monitor thread: readings -> history -> views -> UI + tray.
            let h = handle.clone();
            std::thread::Builder::new()
                .name("monitor".into())
                .spawn(move || {
                    monitor::run(rx, |readings| {
                        #[cfg(debug_assertions)]
                        eprintln!("readings: {readings:?}");
                        let ctx = h.state::<Arc<Ctx>>();
                        let t = now();
                        let (views, toasts) = {
                            let mut st = ctx.state.lock().unwrap();
                            for r in &readings {
                                st.tracker.update(r, t);
                            }
                            st.views = st.tracker.views(t);
                            if t - st.last_save > 120 {
                                st.last_save = t;
                                save("history.json", &st.tracker);
                                save("settings.json", &st.settings);
                            }
                            let views = st.views.clone();
                            let toasts: Vec<_> = views.iter().filter_map(|v| st.alerts.check(v)).collect();
                            (views, toasts)
                        };
                        for (title, body) in toasts {
                            use tauri_plugin_notification::NotificationExt;
                            let _ = h.notification().builder().title(title).body(body).show();
                        }
                        let _ = h.emit_to("panel", "devices", &views);
                        let h2 = h.clone();
                        let _ = h.run_on_main_thread(move || {
                            let ctx = h2.state::<Arc<Ctx>>().inner().clone();
                            let mut st = ctx.state.lock().unwrap();
                            sync_trays(&h2, &ctx, &mut st);
                        });
                    });
                })?;

            if settings.mode == Mode::Floating || first_run {
                let h = handle.clone();
                std::thread::spawn(move || {
                    // Let the first poll land so the panel opens with data.
                    std::thread::sleep(Duration::from_millis(900));
                    let h2 = h.clone();
                    let _ = h.run_on_main_thread(move || show_panel(&h2));
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri app")
        .run(|app, ev| {
            if let tauri::RunEvent::ExitRequested { api, code, .. } = ev {
                // Closing the panel must not quit the tray app; only an explicit exit does.
                if code.is_none() {
                    api.prevent_exit();
                } else {
                    persist(app);
                }
            }
        });
}
