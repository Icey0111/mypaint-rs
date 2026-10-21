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

/// Windows 设备友好名（`DEVPKEY_Device_FriendlyName`）。
///
/// `persistent_identifier()` 给的是设备接口路径；`CM_Get_Device_Interface_PropertyW`
/// 可以直接按接口路径取 PnP 友好名（如 "Wacom Intuos Pro M"）。
#[cfg(windows)]
fn friendly_device_name(id: &str) -> Option<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::CM_Get_Device_Interface_PropertyW;
    use windows_sys::Win32::Devices::Properties::{
        DEVPKEY_Device_FriendlyName, DEVPROP_TYPE_STRING,
    };

    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let mut ty = 0u32;
    let cr = unsafe {
        CM_Get_Device_Interface_PropertyW(
            wide.as_ptr(),
            &DEVPKEY_Device_FriendlyName,
            &mut ty,
            buf.as_mut_ptr().cast::<u8>(),
            &mut size,
            0,
        )
    };
    if cr != 0 || ty != DEVPROP_TYPE_STRING {
        return None;
    }
    let units = (size as usize / 2).min(buf.len());
    let name = String::from_utf16_lossy(&buf[..units]);
    let name = name.trim_end_matches('\0').trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(not(windows))]
fn friendly_device_name(_id: &str) -> Option<String> {
    None
}

/// 设备 id 的短标签。
///
/// Windows HID 接口路径里带 `VID_xxxx&PID_yyyy`：已知厂商映射成名字
/// （Wacom/Huion/XP-Pen），否则显示 `VID:PID`；没有 VID/PID 时取最后一段。
pub(crate) fn device_label(id: &str) -> String {
    if let Some(name) = friendly_device_name(id) {
        return name;
    }
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

/// `gui/device.py::ScrollAction`（`:115-131`）：设备的滚轮默认动作。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScrollAction {
    /// 滚轮缩放（默认）。
    #[default]
    Zoom,
    /// 滚轮平移。
    Pan,
}

impl ScrollAction {
    pub(crate) const VALUES: [Self; 2] = [Self::Zoom, Self::Pan];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Zoom => "zoom",
            Self::Pan => "pan",
        }
    }

    pub(crate) fn from_str(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("pan") {
            Self::Pan
        } else {
            Self::Zoom
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Zoom => "Zoom",
            Self::Pan => "Pan",
        }
    }
}

/// MyPaint 的 per-device 键：`<name>:<source>:<n_axes>`（`device.py:655-660`）。
///
/// winit 不暴露 source/n_axes；数位板笔统一按 `pen:5` 处理。
pub(crate) fn mypaint_device_key(id: &str) -> String {
    format!("{}:pen:5", device_label(id))
}

/// 读 `input.devices[<key>][sub]`。
pub(crate) fn device_pref<'a>(
    prefs: &'a pigment_engine::prefs::Prefs,
    id: &str,
    sub: &str,
) -> Option<&'a str> {
    let key = mypaint_device_key(id);
    prefs
        .get_object("input.devices")?
        .get(&key)?
        .get(sub)?
        .as_str()
}

/// 写/删 `input.devices[<key>][sub]`，保持 MyPaint 的嵌套对象形状。
pub(crate) fn set_device_pref(
    prefs: &mut pigment_engine::prefs::Prefs,
    id: &str,
    sub: &str,
    value: Option<&str>,
) {
    use serde_json::{Map, Value};
    let key = mypaint_device_key(id);
    let mut devices = prefs
        .get_object("input.devices")
        .cloned()
        .unwrap_or_default();
    let mut entry: Map<String, Value> = devices
        .get(&key)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    match value {
        Some(v) => {
            entry.insert(sub.to_string(), Value::String(v.to_string()));
        }
        None => {
            entry.remove(sub);
        }
    }
    if entry.is_empty() {
        devices.remove(&key);
    } else {
        devices.insert(key, Value::Object(entry));
    }
    prefs.set("input.devices", Value::Object(devices));
}

/// MyPaint 的 per-device 笔刷文件名：`devbrush_<uuid5(device_name)>`
/// （`brushmanager.py:64,82-94`）。
pub(crate) fn device_brush_key(id: &str) -> String {
    let name = device_label(id);
    format!("devbrush_{}", uuid5(DEVBRUSH_NAMESPACE, &name))
}

/// `_DEVICE_NAME_NAMESPACE`（`brushmanager.py:72`）。
const DEVBRUSH_NAMESPACE: [u8; 16] = [
    0x16, 0x9e, 0xaf, 0x8a, 0x55, 0x4e, 0x45, 0xb8, 0x82, 0x95, 0xfc, 0x09, 0xb1, 0x00, 0x31, 0xcc,
];

fn uuid5(namespace: [u8; 16], name: &str) -> String {
    let mut data = namespace.to_vec();
    data.extend_from_slice(name.as_bytes());
    let digest = sha1(&data);
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
}

/// 最小 SHA-1（UUID5 只需要它；不引入额外依赖）。
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
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
        assert_eq!(device_label(r"\\?\HID#VID_1234&PID_5678#abc"), "1234:5678");
        assert_eq!(device_label("#42"), "42");
    }

    #[test]
    fn device_override_wins_and_falls_back_to_global() {
        assert_eq!(resolve_usage(Some("ignored"), "any"), TabletUsage::Ignored);
        assert_eq!(resolve_usage(None, "navonly"), TabletUsage::NavOnly);
        assert_eq!(resolve_usage(Some("bogus"), "nopaint"), TabletUsage::Any);
    }

    #[test]
    fn device_brush_filename_uses_mypaint_uuid5() {
        // brushmanager.py:85-87 的官方 doctest 向量。
        assert_eq!(
            uuid5(DEVBRUSH_NAMESPACE, "Wacom Intuos5 touch S Pen stylus"),
            "e97830e9-f9f9-50a5-8fff-68bead1a7021"
        );
    }

    #[test]
    fn scroll_actions_and_mypaint_device_keys() {
        assert_eq!(ScrollAction::from_str("pan"), ScrollAction::Pan);
        assert_eq!(ScrollAction::from_str("ZOOM"), ScrollAction::Zoom);
        assert_eq!(ScrollAction::Pan.as_str(), "pan");
        assert_eq!(
            mypaint_device_key(r"\\?\HID#VID_056A&PID_0374#abc"),
            "Wacom 056A:0374:pen:5"
        );
    }

    #[test]
    fn device_prefs_use_the_native_nested_schema() {
        let mut prefs =
            pigment_engine::prefs::Prefs::from_values(std::collections::BTreeMap::new());
        let id = r"\\?\HID#VID_056A&PID_0374#abc";
        set_device_pref(&mut prefs, id, "usage", Some("ignored"));
        set_device_pref(&mut prefs, id, "scroll", Some("pan"));
        assert_eq!(device_pref(&prefs, id, "usage"), Some("ignored"));
        assert_eq!(device_pref(&prefs, id, "scroll"), Some("pan"));
        let devices = prefs.get_object("input.devices").expect("嵌套对象");
        assert!(devices.contains_key("Wacom 056A:0374:pen:5"));
        set_device_pref(&mut prefs, id, "usage", None);
        assert_eq!(device_pref(&prefs, id, "usage"), None);
        assert_eq!(device_pref(&prefs, id, "scroll"), Some("pan"));
        set_device_pref(&mut prefs, id, "scroll", None);
        assert!(prefs.get_object("input.devices").unwrap().is_empty());
    }

    #[test]
    fn only_any_allows_painting() {
        assert!(TabletUsage::Any.allows_painting());
        for u in [
            TabletUsage::Ignored,
            TabletUsage::NoPaint,
            TabletUsage::NavOnly,
        ] {
            assert!(!u.allows_painting(), "{u:?}");
        }
    }
}
