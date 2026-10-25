//! 菜单栏 / 任务栏托盘图标：小猫睡着 = 未连接，睁眼 = 已连接。窗口图标也跟着变。

use crate::app::AppState;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

const TRAY_ID: &str = "main";

/// 菜单项，切换语言时改文字
struct Items {
    show: MenuItem<tauri::Wry>,
    connect: MenuItem<tauri::Wry>,
    disconnect: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}
static ITEMS: std::sync::OnceLock<Items> = std::sync::OnceLock::new();

fn tooltip(on: bool) -> String {
    if on {
        let app = crate::edition::name();
        tr!("{app} · 已连接", "{app} · Connected")
    } else {
        let app = crate::edition::name();
        tr!("{app} · 未连接", "{app} · Not connected")
    }
}

/// 语言切换后更新托盘菜单和提示文字
pub fn relabel(app: &AppHandle) {
    if let Some(i) = ITEMS.get() {
        let app_name = crate::edition::name();
        let _ = i.show.set_text(tr!("打开 {app_name}", "Open {app_name}"));
        let _ = i.connect.set_text(tr!("连接", "Connect"));
        let _ = i.disconnect.set_text(tr!("断开", "Disconnect"));
        let _ = i.quit.set_text(tr!("退出", "Quit"));
    }
    *LAST.lock().unwrap() = None;
    refresh(app);
}

fn tray_icon(on: bool) -> Image<'static> {
    let bytes: &'static [u8] =
        if on { include_bytes!("../icons/tray-awake.png") } else { include_bytes!("../icons/tray-sleep.png") };
    Image::from_bytes(bytes).expect("tray icon")
}

fn window_icon(on: bool) -> Image<'static> {
    // 科研版：绿色背景的小猫
    let bytes: &'static [u8] = match (crate::edition::is_research(), on) {
        (false, true) => include_bytes!("../icons/window-awake.png"),
        (false, false) => include_bytes!("../icons/window-sleep.png"),
        (true, true) => include_bytes!("../icons-research/window-awake.png"),
        (true, false) => include_bytes!("../icons-research/window-sleep.png"),
    };
    Image::from_bytes(bytes).expect("window icon")
}

/// Mac：窗口收起时只留菜单栏小猫，不占程序坞；打开窗口时再回到程序坞
#[allow(unused_variables)]
fn set_dock_visible(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let policy = if visible { tauri::ActivationPolicy::Regular } else { tauri::ActivationPolicy::Accessory };
        let _ = app.set_activation_policy(policy);
    }
}

/// 收起到菜单栏（Mac）/ 任务栏右下角托盘（Windows）
pub fn hide_to_tray(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    set_dock_visible(app, false);
}

pub fn show_window(app: &AppHandle) {
    set_dock_visible(app, true);
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
    let app_name = crate::edition::name();
    let show = MenuItem::with_id(app, "show", tr!("打开 {app_name}", "Open {app_name}"), true, None::<&str>)?;
    let connect = MenuItem::with_id(app, "connect", tr!("连接", "Connect"), true, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, "disconnect", tr!("断开", "Disconnect"), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", tr!("退出", "Quit"), true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &sep1, &connect, &disconnect, &sep2, &quit])?;
    let _ = ITEMS.set(Items { show: show.clone(), connect: connect.clone(), disconnect: disconnect.clone(), quit: quit.clone() });

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon(false))
        .tooltip(tooltip(false))
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
        let _ = tray.set_tooltip(Some(tooltip(on)));
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_icon(window_icon(on));
    }
}
