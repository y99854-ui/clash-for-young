// 发布版在 Windows 上不弹出命令行窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod edition;
mod app;
mod catrain;
mod check;
mod configgen;
mod core;
mod mobile;
mod settings;
mod sites;
mod subscription;
mod sysproxy_ctl;
mod termproxy;
mod tray;
mod tun;
mod updater;

use app::AppState;
use std::time::Duration;
use tauri::{Manager, RunEvent, WindowEvent};

fn main() {
    tauri::Builder::default()
        // 只允许运行一个：再次打开时把已有的窗口叫出来。
        // 同时开多个会共用同一个内核和配置，互相关掉对方的内核，导致断网。
        // 必须是第一个注册的插件。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|a| {
            let dir = a.path().app_data_dir().expect("无法确定数据目录");
            let geo = a.path().resource_dir().ok().map(|r| r.join("geo"));
            a.manage(AppState::new(dir, geo));
            tray::setup(a.handle())?;
            updater::spawn_background(a.handle().clone());
            let handle = a.handle().clone();
            tauri::async_runtime::spawn(async move {
                let st = handle.state::<AppState>();
                app::on_startup(&st).await;
                // 定时更新订阅：间隔在设置里调（默认 24 小时，0 = 只手动更新）。
                // 按“上次更新时间”判断而不是固定计时，电脑睡眠醒来后也能及时补上。
                loop {
                    let due = {
                        let s = st.settings.lock().unwrap();
                        let interval_ms = s.sub_update_hours as i64 * 3600 * 1000;
                        s.sub_update_hours > 0
                            && s.subscriptions.iter().any(|x| !x.url.is_empty() && settings::now_ms() - x.updated_at > interval_ms)
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
            // 只管主窗口；彩蛋的小猫窗口不走这里
            if window.label() != "main" {
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
            app::set_tun,
            app::relaunch_as_admin,
            app::export_mobile_config,
            app::set_global_mode,
            app::check_update,
            app::install_update,
            app::set_language,
            app::set_campus_mode,
            app::set_terminal_proxy,
            app::set_sub_update_hours,
            catrain::cat_spawn,
            catrain::cat_overlay_ready,
            catrain::cat_overlay_done,
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
