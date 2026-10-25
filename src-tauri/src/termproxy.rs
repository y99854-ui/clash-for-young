//! 科研版：终端代理。连接时让新打开的终端走代理（git、pip、conda、Hugging Face、curl 等都认这几个环境变量），
//! 断开时撤销，避免没开软件时终端连不上网。
//! - Mac / Linux：在 ~/.zshrc（和已有的 ~/.bash_profile、~/.bashrc）末尾加一段带标记的 export，断开时整段删掉。
//! - Windows：写用户环境变量 HTTP_PROXY / HTTPS_PROXY / NO_PROXY，断开时删掉；用户原来自己设过的值会先备份、断开时恢复。
//! 软件崩溃没来得及撤销的，下次打开软件时会清理（on_startup → disable）。

use std::path::{Path, PathBuf};

const NO_PROXY: &str = "localhost,127.0.0.1,::1,.local";

fn marker_begin() -> String {
    format!("# >>> {} terminal proxy (added automatically, removed on disconnect) >>>", crate::edition::name())
}
fn marker_end() -> String {
    format!("# <<< {} terminal proxy <<<", crate::edition::name())
}

pub fn proxy_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// 一段 shell 设置
fn shell_block(port: u16) -> String {
    let u = proxy_url(port);
    format!(
        "{begin}\nexport http_proxy={u} https_proxy={u} HTTP_PROXY={u} HTTPS_PROXY={u}\nexport no_proxy={NO_PROXY} NO_PROXY={NO_PROXY}\n{end}\n",
        begin = marker_begin(),
        end = marker_end()
    )
}

/// 去掉文件内容里我们加的那一段（可能有多段，都去掉）
fn strip_block(text: &str) -> String {
    let (b, e) = (marker_begin(), marker_end());
    let mut out = String::new();
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let l = line.trim_end();
        if l == b {
            skipping = true;
            continue;
        }
        if skipping {
            if l == e {
                skipping = false;
            }
            continue;
        }
        out.push_str(line);
    }
    out
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// 要改的 shell 配置文件：.zshrc 总是改（Mac 默认 zsh，没有就新建）；bash 的只改已经存在的
fn shell_files(home: &Path) -> Vec<PathBuf> {
    let mut v = vec![home.join(".zshrc")];
    for f in [".bash_profile", ".bashrc"] {
        let p = home.join(f);
        if p.exists() {
            v.push(p);
        }
    }
    v
}

fn write_shell(home: &Path, port: Option<u16>) -> Result<(), String> {
    for f in shell_files(home) {
        let old = std::fs::read_to_string(&f).unwrap_or_default();
        let mut new = strip_block(&old);
        if let Some(p) = port {
            if !new.is_empty() && !new.ends_with('\n') {
                new.push('\n');
            }
            new.push_str(&shell_block(p));
        } else if new == old {
            continue; // 没有我们加的内容，不动
        }
        if port.is_none() && !f.exists() {
            continue;
        }
        std::fs::write(&f, new)
            .map_err(|e| crate::tr!("无法修改 {}：{e}", "Can't update {}: {e}", f.display()))?;
    }
    Ok(())
}

/// 连接后：让新终端走代理
pub fn enable(data_dir: &Path, port: u16) -> Result<(), String> {
    if cfg!(windows) {
        win::set(data_dir, port)
    } else {
        let h = home().ok_or("HOME")?;
        write_shell(&h, Some(port))
    }
}

/// 断开 / 启动时：撤销
pub fn disable(data_dir: &Path) -> Result<(), String> {
    if cfg!(windows) {
        win::clear(data_dir)
    } else {
        match home() {
            Some(h) => write_shell(&h, None),
            None => Ok(()),
        }
    }
}

#[cfg(windows)]
mod win {
    //! 用户环境变量在注册表 HKCU\Environment；改完广播 WM_SETTINGCHANGE，新开的终端就能读到
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    const VARS: [&str; 3] = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY"];
    const NO_WINDOW: u32 = 0x0800_0000;

    fn backup_path(dir: &Path) -> std::path::PathBuf {
        dir.join("terminal-proxy-backup.json")
    }

    fn read(name: &str) -> Option<String> {
        let out = Command::new("reg")
            .args(["query", r"HKCU\Environment", "/v", name])
            .creation_flags(NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines().find_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(name)?.trim_start();
            let rest = rest.strip_prefix("REG_SZ").or_else(|| rest.strip_prefix("REG_EXPAND_SZ"))?;
            Some(rest.trim().to_string())
        })
    }

    fn write(name: &str, value: Option<&str>) -> Result<(), String> {
        let mut c = Command::new("reg");
        match value {
            Some(v) => c.args(["add", r"HKCU\Environment", "/v", name, "/t", "REG_SZ", "/d", v, "/f"]),
            None => c.args(["delete", r"HKCU\Environment", "/v", name, "/f"]),
        };
        let out = c.creation_flags(NO_WINDOW).output().map_err(|e| e.to_string())?;
        if value.is_some() && !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(())
    }

    fn broadcast() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };
        let env: Vec<u16> = "Environment\0".encode_utf16().collect();
        let mut result = 0usize;
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                env.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                2000,
                &mut result,
            );
        }
    }

    pub fn set(dir: &Path, port: u16) -> Result<(), String> {
        let ours = super::proxy_url(port);
        // 用户原来自己设过的值先备份（已经是我们写的就不备份）
        if !backup_path(dir).exists() {
            let mut b = serde_json::Map::new();
            for v in VARS {
                if let Some(old) = read(v) {
                    if !old.starts_with("http://127.0.0.1:") && old != super::NO_PROXY {
                        b.insert(v.into(), old.into());
                    }
                }
            }
            let _ = std::fs::write(backup_path(dir), serde_json::Value::Object(b).to_string());
        }
        write("HTTP_PROXY", Some(&ours))?;
        write("HTTPS_PROXY", Some(&ours))?;
        write("NO_PROXY", Some(super::NO_PROXY))?;
        broadcast();
        Ok(())
    }

    pub fn clear(dir: &Path) -> Result<(), String> {
        let Ok(text) = std::fs::read_to_string(backup_path(dir)) else {
            return Ok(()); // 没设过
        };
        let backup: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).unwrap_or_default();
        for v in VARS {
            match backup.get(v).and_then(|x| x.as_str()) {
                Some(old) => write(v, Some(old))?,
                None => write(v, None)?,
            }
        }
        let _ = std::fs::remove_file(backup_path(dir));
        broadcast();
        Ok(())
    }
}

#[cfg(not(windows))]
mod win {
    use std::path::Path;
    pub fn set(_: &Path, _: u16) -> Result<(), String> {
        Ok(())
    }
    pub fn clear(_: &Path) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_remove_block() {
        let dir = std::env::temp_dir().join(format!("cfy-term-{}", crate::settings::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let mine = "export PATH=/opt/bin:$PATH\nalias ll='ls -l'";
        std::fs::write(dir.join(".zshrc"), mine).unwrap();
        std::fs::write(dir.join(".bash_profile"), "source ~/.bashrc\n").unwrap();

        write_shell(&dir, Some(7900)).unwrap();
        write_shell(&dir, Some(7901)).unwrap(); // 再写一次：只留一段，端口更新
        let z = std::fs::read_to_string(dir.join(".zshrc")).unwrap();
        assert!(z.starts_with(mine));
        assert_eq!(z.matches("terminal proxy (added").count(), 1);
        assert!(z.contains("https_proxy=http://127.0.0.1:7901") && !z.contains("7900"));
        assert!(std::fs::read_to_string(dir.join(".bash_profile")).unwrap().contains("7901"));
        assert!(!dir.join(".bashrc").exists(), "不存在的 bash 配置不新建");

        write_shell(&dir, None).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(".zshrc")).unwrap(), format!("{mine}\n"));
        assert_eq!(std::fs::read_to_string(dir.join(".bash_profile")).unwrap(), "source ~/.bashrc\n");

        // 真的在 zsh/bash 里生效
        write_shell(&dir, Some(7902)).unwrap();
        let out = std::process::Command::new("bash")
            .args(["-c", &format!("source {}/.zshrc 2>/dev/null; echo $https_proxy $NO_PROXY", dir.display())])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("http://127.0.0.1:7902 {NO_PROXY}"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
