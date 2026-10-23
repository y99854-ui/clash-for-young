//! 界面语言：中文 / English。设置里的 language 为空表示跟随系统（系统是中文就用中文，否则用英文）。
//! 后端给界面的提示、错误、托盘菜单都用 tr!("中文", "English") 按当前语言输出。

use std::sync::atomic::{AtomicBool, Ordering};

static EN: AtomicBool = AtomicBool::new(false);

pub fn is_en() -> bool {
    EN.load(Ordering::Relaxed)
}

/// 系统语言是不是中文
pub fn system_is_zh() -> bool {
    sys_locale::get_locale().map(|l| l.to_lowercase().starts_with("zh")).unwrap_or(true)
}

/// 按设置决定语言。setting：""（跟随系统）/ "zh" / "en"。返回实际使用的 "zh" 或 "en"。
pub fn apply(setting: &str) -> &'static str {
    let en = match setting {
        "en" => true,
        "zh" => false,
        _ => !system_is_zh(),
    };
    EN.store(en, Ordering::Relaxed);
    current()
}

pub fn current() -> &'static str {
    if is_en() {
        "en"
    } else {
        "zh"
    }
}

/// tr!("中文 {x}", "English {x}") → 按当前语言格式化成 String
#[macro_export]
macro_rules! tr {
    ($zh:literal, $en:literal $(,)?) => {
        if $crate::i18n::is_en() { format!($en) } else { format!($zh) }
    };
    ($zh:literal, $en:literal, $($arg:tt)+) => {
        if $crate::i18n::is_en() { format!($en, $($arg)+) } else { format!($zh, $($arg)+) }
    };
}

#[cfg(test)]
pub fn set_en_for_test(en: bool) {
    EN.store(en, Ordering::Relaxed);
}
