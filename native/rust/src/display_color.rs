//! 输出色彩空间：鸿蒙侧仅决定窗口走 Display P3 还是 sRGB（由 C 边界 `gc_set_window_color_space` 落实），
//! 不做像素级色彩变换（首版移植的最小实现；后续可接入 craft 家族的 ICC 链路）。
use egui::Color32;

#[derive(Clone, Copy)]
pub struct DisplayColor {
    pub p3: bool,
}

impl DisplayColor {
    pub fn new(p3: bool) -> Result<Self, String> {
        Ok(Self { p3 })
    }

    /// UI 顶点着色：首版不做转换，直接返回原色。
    pub fn ui_color(&self, color: Color32) -> Color32 {
        color
    }

    /// 纹理上传前的色彩处理：首版为 no-op。
    #[allow(dead_code)]
    pub fn texture(&self, _id: egui::TextureId, _image: &mut egui::epaint::ImageData) {}
}
