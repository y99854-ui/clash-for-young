// 发布版在 Windows 上不弹出命令行窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod check;
mod configgen;
mod core;
mod settings;
mod sites;
mod subscription;
mod sysproxy_ctl;
mod tray;

use app::AppState;
use std::time::Duration;
use tauri::{Manager, RunEvent, WindowEvent};

/// 订阅多久自动更新一次
const AUTO_UPDATE: Duration = Duration::from_secs(6 * 3600);

fn main() {
    tauri::Builder::default()
        .setup(|a| {
            let dir = a.path().app_data_dir().expect("无法确定数据目录");
            let geo = a.path().resource_dir().ok().map(|r| r.join("geo"));
            a.manage(AppState::new(dir, geo));
            tray::setup(a.handle())?;
            let handle = a.handle().clone();
            tauri::async_runtime::spawn(async move {
                let st = handle.state::<AppState>();
                app::on_startup(&st).await;
                // 定时更新订阅
                loop {
                    let due = {
                        let s = st.settings.lock().unwrap();
                        s.subscriptions
                            .iter()
                            .any(|x| !x.url.is_empty() && settings::now_ms() - x.updated_at > AUTO_UPDATE.as_millis() as i64)
                    };
                    if due {
                        app::auto_refresh(&st).await;
                    }
                    tokio::time::sleep(Duration::from_secs(600)).await;
                }
            });
            Ok(())
        })
        // 点窗口的关闭按钮只是收起到菜单栏（Mac，程序坞图标也隐藏）/ 托盘（Windows），软件继续在后台运行。
        // 真正退出：托盘菜单「退出」，或 Mac 上 Cmd+Q。
        .on_window_event(|window, event| {
            // Linux 上没有托盘，隐藏后就找不回窗口，所以照常关闭
            if cfg!(target_os = "linux") {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                tray::hide_to_tray(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            app::get_state,
            app::add_subscription,
            app::import_subscription_file,
            app::remove_subscription,
            app::rename_subscription,
            app::refresh_subscription,
            app::connect,
            app::disconnect,
            app::set_default_target,
            app::set_site_choice,
            app::save_site,
            app::delete_site,
            app::set_custom_rules,
            app::status,
            app::node_delays,
            app::check_url,
            app::core_log,
            app::current_config,
            app::set_ports,
            app::preview_group,
            app::save_group,
            app::delete_group,
            app::move_group,
            app::reset_groups,
        ])
        .build(tauri::generate_context!())
        .expect("启动失败")
        .run(|handle, event| {
            // Mac：窗口隐藏后点程序坞里的图标，重新显示窗口
            #[cfg(target_os = "macos")]
            if let RunEvent::Reopen { .. } = event {
                tray::show_window(handle);
            }
            // 退出时恢复系统代理并关闭内核，避免断网
            if let RunEvent::Exit = event {
                if let Some(st) = handle.try_state::<AppState>() {
                    let _ = st.disconnect(false);
                }
            }
        });
}
