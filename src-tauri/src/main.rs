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
use tauri::{Manager, RunEvent};

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
                        s.subscriptions.first().map(|x| settings::now_ms() - x.updated_at > AUTO_UPDATE.as_millis() as i64)
                    };
                    if due == Some(true) {
                        let _ = app::refresh_subscription(handle.state::<AppState>()).await;
                    }
                    tokio::time::sleep(Duration::from_secs(600)).await;
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::get_state,
            app::set_subscription,
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
            // 退出时恢复系统代理并关闭内核，避免断网
            if let RunEvent::Exit = event {
                if let Some(st) = handle.try_state::<AppState>() {
                    let _ = st.disconnect(false);
                }
            }
        });
}
