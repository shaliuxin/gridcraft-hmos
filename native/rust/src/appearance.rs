//! 系统主题：ArkTS 上报明/暗，作为 egui `RawInput.system_theme` 的来源（None 表示跟随系统）。
use egui::Theme;
use std::sync::atomic::{AtomicI8, Ordering};

/// 0 = 未定（跟随系统），1 = 暗，-1 = 亮。
static SYSTEM_THEME: AtomicI8 = AtomicI8::new(0);

pub fn set_dark(dark: bool) {
    SYSTEM_THEME.store(if dark { 1 } else { -1 }, Ordering::Release);
}

/// egui 的 `system_theme`：`Some(Theme::Dark/Light)` 明确指定，None 表示让系统决定。
pub fn current() -> Option<Theme> {
    match SYSTEM_THEME.load(Ordering::Acquire) {
        1 => Some(Theme::Dark),
        -1 => Some(Theme::Light),
        _ => None,
    }
}
