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

/// Mac：授权后的内核副本放在这里（root 所有、setuid）。
/// 不能直接改安装位置的文件：macOS 不允许管理员进程修改「文稿」等受保护文件夹里的文件，
/// 而且软件更新会把它覆盖掉。
pub fn mac_priv_dir() -> &'static str {
    crate::edition::mac_priv_dir()
}

pub fn mac_priv_bin() -> std::path::PathBuf {
    Path::new(mac_priv_dir()).join("mihomo")
}

/// 增强模式下实际启动的内核文件：Mac 用授权过的副本，其他系统用安装包里的
pub fn launch_binary() -> Option<std::path::PathBuf> {
    if cfg!(target_os = "macos") {
        Some(mac_priv_bin())
    } else {
        None
    }
}

/// 两个文件内容是否相同（比较大小和首尾各 64KB，足够判断是不是同一个版本）
#[allow(dead_code)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    let chunk = |p: &Path, from_end: bool| -> Option<Vec<u8>> {
        let mut f = std::fs::File::open(p).ok()?;
        let len = f.metadata().ok()?.len();
        let n = len.min(65536);
        if from_end {
            f.seek(SeekFrom::Start(len - n)).ok()?;
        }
        let mut buf = vec![0u8; n as usize];
        f.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    chunk(a, false) == chunk(b, false) && chunk(a, true) == chunk(b, true)
}

/// 内核现在是否有权限开启 TUN
#[allow(unused_variables)]
pub fn privileged(bin: Option<&Path>) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::MetadataExt;
        let p = mac_priv_bin();
        let ok = std::fs::metadata(&p).map(|m| m.uid() == 0 && m.mode() & 0o4000 != 0).unwrap_or(false);
        // 软件更新后安装包里的内核变了，副本就过期了，需要重新授权
        return ok && bin.map(|b| same_file(b, &p)).unwrap_or(false);
    }
    #[cfg(windows)]
    {
        return unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() } != 0;
    }
    #[allow(unreachable_code)]
    false
}

/// Mac：弹出系统密码框，把内核复制到 mac_priv_dir() 并授权。
/// stage：先由本软件（普通权限）把内核复制到这个临时位置，管理员命令再从这里拷走，
/// 避免管理员进程去读受保护的文件夹。
#[allow(unused_variables)]
pub fn grant(bin: &Path, stage: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::fs::copy(bin, stage).map_err(|e| crate::tr!("无法准备内核文件：{e}", "Can't prepare the core file: {e}"))?;
        let q = |p: &Path| {
            p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"").replace('\'', "'\\\\''")
        };
        let dst = mac_priv_bin();
        let cmd = format!(
            "mkdir -p '{dir}' && cp -f '{src}' '{dst}' && chown root:wheel '{dst}' && chmod 4755 '{dst}'",
            dir = q(Path::new(mac_priv_dir())),
            src = q(stage),
            dst = q(&dst),
        );
        let app = crate::edition::name();
        let prompt = crate::tr!(
            "{app} 需要管理员权限来开启增强模式（TUN）。",
            "{app} needs administrator permission to turn on Enhanced mode (TUN)."
        );
        let script = format!("do shell script \"{cmd}\" with prompt \"{prompt}\" with administrator privileges");
        let out = std::process::Command::new("osascript")
            .current_dir("/")
            .args(["-e", &script])
            .output()
            .map_err(|e| crate::tr!("无法请求管理员权限：{e}", "Can't request administrator permission: {e}"));
        let _ = std::fs::remove_file(stage);
        let out = out?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") || err.to_lowercase().contains("cancel") {
            return Err(crate::tr!("已取消授权。", "Authorization cancelled."));
        }
        return Err(crate::tr!("授权失败：{}", "Authorization failed: {}", err.trim()));
    }
    #[allow(unreachable_code)]
    Err(crate::tr!("当前系统不支持这种授权方式。", "This system doesn't support this kind of authorization."))
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
        return Err(crate::tr!("没有获得管理员权限（可能在弹窗里点了“否”）。", "Didn't get administrator permission (you may have clicked \"No\" in the prompt)."));
    }
    #[allow(unreachable_code)]
    Err(crate::tr!("只有 Windows 需要这样做。", "This is only needed on Windows."))
}
