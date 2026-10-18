//! 设备策略（`gui/device.py` 的 `AllowedUsage` 的可实现子集）。
//!
//! winit 0.31 不暴露设备名（`DeviceId` 是不透明 i64），拿不到 MyPaint 那套
//! 按设备名分组的设置。这里退到**按工具类别**的全局策略：所有数位板工具
//! 共用一条 usage，鼠标不受影响；设备名/逐设备设置记为平台限制。

/// `gui/device.py::AllowedUsage` 的四种取值。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TabletUsage {
    /// 可绘画、可导航。
    #[default]
    Any,
    /// 完全忽略（不绘画、不导航）。
    Ignored,
    /// 不能直接绘画，但可操作对象/导航。
    NoPaint,
    /// 只能导航。
    NavOnly,
}

impl TabletUsage {
    pub(crate) const VALUES: [Self; 4] = [Self::Any, Self::Ignored, Self::NoPaint, Self::NavOnly];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Ignored => "ignored",
            Self::NoPaint => "nopaint",
            Self::NavOnly => "navonly",
        }
    }

    /// 未知值回到默认 `Any`。
    pub(crate) fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "ignored" => Self::Ignored,
            "nopaint" => Self::NoPaint,
            "navonly" => Self::NavOnly,
            _ => Self::Any,
        }
    }

    /// 是否允许在画布上落笔。
    pub(crate) fn allows_painting(self) -> bool {
        self == Self::Any
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Any => "Any Task",
            Self::Ignored => "Ignore",
            Self::NoPaint => "Non-painting tasks",
            Self::NavOnly => "Navigation only",
        }
    }
}

/// 设备 id 的短标签。
///
/// Windows HID 接口路径里带 `VID_xxxx&PID_yyyy`：已知厂商映射成名字
/// （Wacom/Huion/XP-Pen），否则显示 `VID:PID`；没有 VID/PID 时取最后一段。
pub(crate) fn device_label(id: &str) -> String {
    let Some(seg) = id.split('#').find(|s| s.starts_with("VID_")) else {
        return id.rsplit('#').next().unwrap_or(id).to_string();
    };
    let mut vid = None;
    let mut pid = None;
    for part in seg.split('&') {
        if let Some(v) = part.strip_prefix("VID_") {
            vid = Some(v.to_ascii_uppercase());
        } else if let Some(p) = part.strip_prefix("PID_") {
            pid = Some(p.to_ascii_uppercase());
        }
    }
    let (Some(vid), Some(pid)) = (vid, pid) else {
        return id.rsplit('#').next().unwrap_or(id).to_string();
    };
    let vendor = match vid.as_str() {
        "056A" => "Wacom",
        "256C" => "Huion",
        "28BD" => "XP-Pen",
        _ => "",
    };
    if vendor.is_empty() {
        format!("{vid}:{pid}")
    } else {
        format!("{vendor} {vid}:{pid}")
    }
}

/// 逐设备覆盖优先，缺失时回落到全局策略。
pub(crate) fn resolve_usage(specific: Option<&str>, global: &str) -> TabletUsage {
    specific
        .map(TabletUsage::from_str)
        .unwrap_or_else(|| TabletUsage::from_str(global))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_values_and_defaults_to_any() {
        assert_eq!(TabletUsage::from_str("any"), TabletUsage::Any);
        assert_eq!(TabletUsage::from_str("IGNORED"), TabletUsage::Ignored);
        assert_eq!(TabletUsage::from_str("nopaint"), TabletUsage::NoPaint);
        assert_eq!(TabletUsage::from_str("navonly"), TabletUsage::NavOnly);
        assert_eq!(TabletUsage::from_str("garbage"), TabletUsage::Any);
        assert_eq!(TabletUsage::default(), TabletUsage::Any);
    }

    #[test]
    fn device_labels_shorten_windows_paths() {
        assert_eq!(
            device_label(r"\\?\HID#VID_056A&PID_0374#abc"),
            "Wacom 056A:0374"
        );
        assert_eq!(
            device_label(r"\\?\HID#VID_1234&PID_5678#abc"),
            "1234:5678"
        );
        assert_eq!(device_label("#42"), "42");
    }

    #[test]
    fn device_override_wins_and_falls_back_to_global() {
        assert_eq!(resolve_usage(Some("ignored"), "any"), TabletUsage::Ignored);
        assert_eq!(resolve_usage(None, "navonly"), TabletUsage::NavOnly);
        assert_eq!(resolve_usage(Some("bogus"), "nopaint"), TabletUsage::Any);
    }

    #[test]
    fn only_any_allows_painting() {
        assert!(TabletUsage::Any.allows_painting());
        for u in [TabletUsage::Ignored, TabletUsage::NoPaint, TabletUsage::NavOnly] {
            assert!(!u.allows_painting(), "{u:?}");
        }
    }
}
