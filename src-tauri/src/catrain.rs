//! 彩蛋：小猫从首页开关的传送门喷出来，飞出软件窗口，在屏幕四边反弹。
//! 做法是开一个铺满当前屏幕、透明、置顶、鼠标可以点穿的窗口（catrain.html）专门画小猫。

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "catrain";

/// 一批小猫：x、y 是喷出点在小猫窗口里的位置（网页像素）
#[derive(Clone, Serialize)]
pub struct Spawn {
    x: f64,
    y: f64,
    n: u32,
}

/// 小猫窗口的网页是否已经加载好；加载好之前的小猫先攒着
static READY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Vec<Spawn>> = Mutex::new(Vec::new());

fn overlay(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Ok(w);
    }
    READY.store(false, Ordering::SeqCst);
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("catrain.html".into()))
        .title("")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = w.set_ignore_cursor_events(true);
    Ok(w)
}

/// 主窗口喷一批小猫。x、y 是传送门中心在主窗口网页里的位置。
/// 返回小猫窗口是否已经加载好，主窗口据此判断要不要退回在本窗口里喷。
/// 必须是 async：Windows 上在同步命令里新建窗口会卡死（Tauri 已知问题 #4121）。
#[tauri::command]
pub async fn cat_spawn(app: AppHandle, window: WebviewWindow, x: f64, y: f64, n: u32) -> Result<bool, String> {
    let err = |e: tauri::Error| e.to_string();
    // 主窗口网页坐标 → 屏幕上的物理像素
    let scale = window.scale_factor().map_err(err)?;
    let inner = window.inner_position().map_err(err)?;
    let px = inner.x as f64 + x * scale;
    let py = inner.y as f64 + y * scale;
    let mon = match window.current_monitor().map_err(err)? {
        Some(m) => m,
        None => window.primary_monitor().map_err(err)?.ok_or("找不到屏幕")?,
    };
    let (mpos, msize, mscale) = (*mon.position(), *mon.size(), mon.scale_factor());

    let ov = overlay(&app)?;
    // 铺满主窗口所在的屏幕；主窗口换了屏幕就跟过去
    let placed = ov.is_visible().unwrap_or(false)
        && ov.outer_position().ok() == Some(mpos)
        && ov.outer_size().ok() == Some(msize);
    if !placed {
        let _ = ov.set_position(PhysicalPosition::new(mpos.x, mpos.y));
        let _ = ov.set_size(PhysicalSize::new(msize.width, msize.height));
        let _ = ov.show();
        let _ = ov.set_ignore_cursor_events(true);
        let _ = ov.set_always_on_top(true);
        // 显示小猫窗口不要抢走主窗口的焦点
        let _ = window.set_focus();
    }

    let s = Spawn { x: (px - mpos.x as f64) / mscale, y: (py - mpos.y as f64) / mscale, n: n.min(10) };
    let ready = READY.load(Ordering::SeqCst);
    if ready {
        app.emit_to(LABEL, "cat-spawn", s).map_err(err)?;
    } else {
        PENDING.lock().unwrap().push(s);
    }
    Ok(ready)
}

/// 小猫窗口加载好了：领走之前攒下的小猫
#[tauri::command]
pub fn cat_overlay_ready() -> Vec<Spawn> {
    READY.store(true, Ordering::SeqCst);
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// 小猫都消失了：收起小猫窗口（不关闭，下次直接用）
#[tauri::command]
pub fn cat_overlay_done(app: AppHandle) {
    PENDING.lock().unwrap().clear();
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.hide();
    }
}
