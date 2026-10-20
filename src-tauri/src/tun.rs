//! 增强模式（TUN）：接管电脑上所有程序的流量，需要管理员权限。
//! - Mac：给内核文件授权一次（setuid root，会弹出系统密码框），之后内核就能创建虚拟网卡。
//!   软件更新会替换内核文件，需要再授权一次。
//! - Windows：整个软件以管理员身份运行，内核继承权限。

use std::path::Path;

pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// 内核现在是否有权限开启 TUN
#[allow(unused_variables)]
pub fn privileged(bin: Option<&Path>) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        return bin
            .and_then(|b| std::fs::metadata(b).ok())
            .map(|m| m.uid() == 0 && m.mode() & 0o4000 != 0)
            .unwrap_or(false);
    }
    #[cfg(windows)]
    {
        return unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() } != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// Mac：弹出系统密码框，给内核文件授权
#[allow(unused_variables)]
pub fn grant(bin: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let p = bin.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"").replace('\'', "'\\\\''");
        let script = format!(
            "do shell script \"chown root:admin '{p}' && chmod u+s,a+x '{p}'\" with prompt \"Clash for Young 需要管理员权限来开启增强模式（TUN）。\" with administrator privileges"
        );
        let out = std::process::Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("无法请求管理员权限：{e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") || err.to_lowercase().contains("cancel") {
            return Err("已取消授权。".into());
        }
        return Err(format!("授权失败：{}", err.trim()));
    }
    #[allow(unreachable_code)]
    Err("当前系统不支持这种授权方式。".into())
}

/// Windows：以管理员身份重新打开本软件（会弹出 UAC 确认）
pub fn relaunch_as_admin() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let verb = wide(std::ffi::OsStr::new("runas"));
        let file = wide(exe.as_os_str());
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // 返回值大于 32 表示成功；用户在 UAC 里点了“否”会失败
        if r as isize > 32 {
            return Ok(());
        }
        return Err("没有获得管理员权限（可能在弹窗里点了“否”）。".into());
    }
    #[allow(unreachable_code)]
    Err("只有 Windows 需要这样做。".into())
}
