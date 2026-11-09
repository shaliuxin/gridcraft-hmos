//! 设备形态：把 ArkTS 上报的 `deviceInfo.deviceType` + 宽度换算成断点档位，与 ArkUI 一致。
//! 设备类型优先于宽度（处理"手机横屏窗口很宽但仍是手机"）。
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceType {
    Phone,
    Tablet,
    Pc,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WidthClass {
    Compact,
    Medium,
    Expanded,
}

#[derive(Clone, Copy, Debug)]
pub struct Form {
    pub device_type: DeviceType,
    pub width_vp: f32,
    pub width_class: WidthClass,
}

impl Form {
    pub fn new(device_type: &str, width_vp: f32) -> Self {
        let device_type = match device_type {
            "phone" => DeviceType::Phone,
            "tablet" => DeviceType::Tablet,
            "2in1" | "pc" => DeviceType::Pc,
            _ => DeviceType::Unknown,
        };
        let width_class = if device_type != DeviceType::Unknown {
            // 设备类型明确时按设备定档：手机/平板紧凑，2in1/PC 宽松。
            match device_type {
                DeviceType::Phone => WidthClass::Compact,
                DeviceType::Tablet => WidthClass::Medium,
                DeviceType::Pc => WidthClass::Expanded,
                DeviceType::Unknown => width_class(width_vp),
            }
        } else {
            width_class(width_vp)
        };
        Self {
            device_type,
            width_vp,
            width_class,
        }
    }
}

fn width_class(width_vp: f32) -> WidthClass {
    if width_vp >= 840.0 {
        WidthClass::Expanded
    } else if width_vp >= 600.0 {
        WidthClass::Medium
    } else {
        WidthClass::Compact
    }
}

/// ArkTS 每次上报设备形态时携带的负载。
#[derive(Deserialize, Default)]
#[allow(dead_code)]
pub struct NativeConfig {
    #[serde(default)]
    pub device_type: String,
    #[serde(default)]
    pub width_vp: f32,
}
