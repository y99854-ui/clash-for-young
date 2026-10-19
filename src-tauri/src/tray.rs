//! 菜单栏 / 任务栏托盘图标：小猫睡着 = 未连接，睁眼 = 已连接。窗口图标也跟着变。

use crate::app::AppState;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

const TRAY_ID: &str = "main";

fn tray_icon(on: bool) -> Image<'static> {
    let bytes: &'static [u8] =
        if on { include_bytes!("../icons/tray-awake.png") } else { include_bytes!("../icons/tray-sleep.png") };
    Image::from_bytes(bytes).expect("tray icon")
}

fn window_icon(on: bool) -> Image<'static> {
    let bytes: &'static [u8] =
        if on { include_bytes!("../icons/window-awake.png") } else { include_bytes!("../icons/window-sleep.png") };
    Image::from_bytes(bytes).expect("window icon")
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    // Linux 上托盘需要 appindicator 库，没有就只换窗口图标
    if !cfg!(target_os = "linux") {
        build_tray(app)?;
    }
    // 后台每秒检查一次连接状态，变了就换图标（连接/断开可能来自界面、托盘或内核意外退出）
    let handle = app.clone();
    std::thread::spawn(move || loop {
        refresh(&handle);
        std::thread::sleep(Duration::from_secs(1));
    });
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开 Clash for Young", true, None::<&str>)?;
    let connect = MenuItem::with_id(app, "connect", "连接", true, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, "disconnect", "断开", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep1, &connect, &disconnect, &sep2, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon(false))
        .tooltip("Clash for Young · 未连接")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "show" => show_window(app),
            "connect" | "disconnect" => {
                let on = e.id().as_ref() == "connect";
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let st = app.state::<AppState>();
                    let _g = st.op.lock().await;
                    let r = if on { st.connect().await } else { st.disconnect(true) };
                    if let Err(err) = r {
                        *st.core.last_error.lock().unwrap() = Some(err);
                    }
                    refresh(&app);
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

static LAST: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

/// 按当前状态更新托盘和窗口图标
pub fn refresh(app: &AppHandle) {
    let Some(st) = app.try_state::<AppState>() else { return };
    let on = st.connected.load(Ordering::SeqCst) && st.core.is_running();
    {
        let mut last = LAST.lock().unwrap();
        if *last == Some(on) {
            return;
        }
        *last = Some(on);
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(tray_icon(on)));
        let _ = tray.set_tooltip(Some(if on { "Clash for Young · 已连接" } else { "Clash for Young · 未连接" }));
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_icon(window_icon(on));
    }
}
