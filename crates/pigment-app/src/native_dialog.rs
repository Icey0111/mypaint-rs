//! 系统原生文件选择器 —— 补上 GTK `Gtk.FileChooserDialog` 的对应物。
//!
//! `gui/filehandling.py:1185-1228` 的 `ImportLayers` 用
//! `dialog.set_select_multiple(True)` + `get_filenames()`。egui 没有原生文件
//! 对话框；这里在 Windows 上直接调 `comdlg32!GetOpenFileNameW`，带
//! `OFN_ALLOWMULTISELECT | OFN_EXPLORER`，返回真正的多选路径。其它平台返回
//! `None`，调用方回退到应用内的路径文本弹层（技术栈适配）。

use std::path::PathBuf;

/// 打开多选文件对话框；取消/失败返回 `None`。
///
/// `filter` 是 Win32 的双 NUL 结尾过滤器；`title` 是对话框标题。
/// 非 Windows 平台恒返回 `None`（见模块文档）。
#[cfg(windows)]
pub(crate) fn pick_files(
    title: &str,
    filter: &str,
    initial_dir: Option<&std::path::Path>,
) -> Option<Vec<PathBuf>> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let mut file_buf = vec![0u16; 65536];
    let filter_w = wide(filter);
    let title_w = wide(title);
    let dir_w: Vec<u16> = initial_dir
        .map(|p| {
            p.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        })
        .unwrap_or_default();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: filter_w.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrTitle: title_w.as_ptr(),
        Flags: OFN_ALLOWMULTISELECT
            | OFN_EXPLORER
            | OFN_FILEMUSTEXIST
            | OFN_PATHMUSTEXIST
            | OFN_HIDEREADONLY,
        lpstrInitialDir: if dir_w.is_empty() {
            std::ptr::null()
        } else {
            dir_w.as_ptr()
        },
        ..Default::default()
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if ok == 0 {
        return None;
    }
    parse_open_file_buffer(&file_buf).map(|paths| {
        paths
            .into_iter()
            .map(|p| {
                if p.is_absolute() || p.components().count() > 1 {
                    p
                } else if let Some(dir) = initial_dir {
                    dir.join(p)
                } else {
                    p
                }
            })
            .collect()
    })
}

#[cfg(not(windows))]
pub(crate) fn pick_files(
    _title: &str,
    _filter: &str,
    _initial_dir: Option<&std::path::Path>,
) -> Option<Vec<PathBuf>> {
    None
}

/// 打开单选文件对话框（`Gtk.FileChooserAction.OPEN` 的对应物）。
#[cfg(windows)]
pub(crate) fn pick_file(
    title: &str,
    filter: &str,
    initial_dir: Option<&std::path::Path>,
) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let mut file_buf = vec![0u16; 32768];
    let filter_w = wide(filter);
    let title_w = wide(title);
    let dir_w: Vec<u16> = initial_dir
        .map(|p| {
            p.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        })
        .unwrap_or_default();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: filter_w.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrTitle: title_w.as_ptr(),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY,
        lpstrInitialDir: if dir_w.is_empty() {
            std::ptr::null()
        } else {
            dir_w.as_ptr()
        },
        ..Default::default()
    };
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    parse_open_file_buffer(&file_buf).and_then(|mut v| v.drain(..).next())
}

#[cfg(not(windows))]
pub(crate) fn pick_file(
    _title: &str,
    _filter: &str,
    _initial_dir: Option<&std::path::Path>,
) -> Option<PathBuf> {
    None
}

/// 保存文件对话框（`Gtk.FileChooserAction.SAVE` + `set_do_overwrite_confirmation`）。
///
/// `default_ext` 不带点（如 `"ora"`），用户没写扩展名时由 Win32 自动补上。
#[cfg(windows)]
pub(crate) fn save_file(
    title: &str,
    filter: &str,
    default_ext: &str,
    suggested: Option<&std::path::Path>,
) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_EXPLORER, OFN_HIDEREADONLY, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let mut file_buf = vec![0u16; 32768];
    if let Some(p) = suggested {
        let name: Vec<u16> = p
            .as_os_str()
            .encode_wide()
            .take(file_buf.len() - 1)
            .collect();
        file_buf[..name.len()].copy_from_slice(&name);
    }
    let filter_w = wide(filter);
    let title_w = wide(title);
    let ext_w = wide(default_ext.trim_start_matches('.'));
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: filter_w.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrTitle: title_w.as_ptr(),
        lpstrDefExt: ext_w.as_ptr(),
        Flags: OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY,
        ..Default::default()
    };
    if unsafe { GetSaveFileNameW(&mut ofn) } == 0 {
        return None;
    }
    parse_open_file_buffer(&file_buf).and_then(|mut v| v.drain(..).next())
}

#[cfg(not(windows))]
pub(crate) fn save_file(
    _title: &str,
    _filter: &str,
    _default_ext: &str,
    _suggested: Option<&std::path::Path>,
) -> Option<PathBuf> {
    None
}

/// 解析 `GetOpenFileNameW` 的返回缓冲区。
///
/// Win32 的格式（`OFN_EXPLORER` + 多选）：
/// - 单选：`C:\dir\file.ora\0`
/// - 多选：`C:\dir\0file1.ora\0file2.ora\0\0`
///
/// 因此先读到第一个 NUL；若其后还有内容，则第一段是目录，其余是文件名。
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_open_file_buffer(buf: &[u16]) -> Option<Vec<PathBuf>> {
    let first_nul = buf.iter().position(|&c| c == 0)?;
    let first = String::from_utf16_lossy(&buf[..first_nul]);
    if first.is_empty() {
        return None;
    }
    let rest = &buf[first_nul + 1..];
    let rest_end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    if rest_end == 0 {
        // 单选：整个缓冲区就是完整路径。
        return Some(vec![PathBuf::from(first)]);
    }
    let dir = PathBuf::from(first);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < rest.len() {
        let end = rest[start..]
            .iter()
            .position(|&c| c == 0)
            .map(|n| start + n)
            .unwrap_or(rest.len());
        if end == start {
            break; // 双 NUL 结束
        }
        let name = String::from_utf16_lossy(&rest[start..end]);
        out.push(dir.join(name));
        start = end + 1;
    }
    if out.is_empty() {
        Some(vec![dir])
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    #[test]
    fn parses_single_selection_as_full_path() {
        let mut buf = utf16(r"C:\art\one.ora");
        buf.push(0); // 结束哨兵
        let paths = parse_open_file_buffer(&buf).unwrap();
        assert_eq!(paths, vec![PathBuf::from(r"C:\art\one.ora")]);
    }

    #[test]
    fn parses_multi_selection_directory_plus_names() {
        let mut buf = utf16(r"C:\art");
        for name in ["one.ora", "two.ora", "three.png"] {
            buf.extend(name.encode_utf16());
            buf.push(0);
        }
        buf.push(0); // 双 NUL
        let paths = parse_open_file_buffer(&buf).unwrap();
        assert_eq!(
            paths,
            vec![
                PathBuf::from(r"C:\art\one.ora"),
                PathBuf::from(r"C:\art\two.ora"),
                PathBuf::from(r"C:\art\three.png"),
            ]
        );
    }

    #[test]
    fn empty_buffer_is_rejected() {
        assert!(parse_open_file_buffer(&[0]).is_none());
        assert!(parse_open_file_buffer(&[]).is_none());
    }
}
