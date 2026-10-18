//! Scrap 文件 —— `gui/filehandling.py:1339-1475` 的 `save_scrap_cb` /
//! `save_autoincrement_file` / `get_scrap_prefix` / `list_scraps`。
//!
//! # 源码的 scrap 模型
//!
//! Scrap 是 MyPaint 的"草稿纸"文件，全部放在偏好 `saving.scrap_prefix`
//! 指向的目录里（Windows 默认 `Documents\MyPaint\scrap`，其它
//! `~/MyPaint/scrap`）。命名规则（`save_autoincrement_file`）：
//!
//! ```text
//! <prefix><三位以上编号>_<a..z>.<扩展名>
//!   001_a.ora      # 第一次 Save As Scrap
//!   001_b.ora      # 对同一 scrap 再 Save As Scrap → 新修订
//!   002_a.ora      # 另一个 scrap
//! ```
//!
//! 编号复用：当前文件名里能解析出 `<prefix><数字>` 时沿用该编号，字母往后
//! 找；`z` 用尽则编号 +1。当前文件名以 `_md5` 开头时**直接覆盖**，不进编号
//! （源码给特殊链接草稿纸留的口子）。
//!
//! `list_scraps` 按扩展名（png/ora/jpg/jpeg，大小写都收）与 `_md5` 前缀
//! 收集，排序后按编号分组；`open_scrap_cb` 在组之间前后跳，落在组内**最后
//! 一个修订**上（`groups[idx][-1]`）。

use std::path::{Path, PathBuf};

/// 收集 scrap 时接受的扩展名（`filehandling.py:1461-1470`）。
pub const SCRAP_EXTENSIONS: [&str; 4] = ["png", "ora", "jpg", "jpeg"];

/// 展开 `~` 与 `~user` 的前缀（`fileutils.expanduser_unicode` 的常用分支）。
///
/// 只处理 `~` 与 `~/`（Windows 的 `~\` 也收）；`~user` 需要系统用户库，
/// 标准库给不了，原样返回并在文档记账。
pub fn expand_user(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = user_home() {
            return home;
        }
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = user_home() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

fn user_home() -> Option<PathBuf> {
    for var in ["USERPROFILE", "HOME"] {
        if let Some(p) = std::env::var_os(var) {
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

/// 当前文件名是否属于 scrap 目录（`set_filename` 的
/// `filename.startswith(get_scrap_prefix())`，`filehandling.py:637-639`）。
pub fn is_inside_scrap_dir(scrap_dir: &Path, path: &Path) -> bool {
    let a = normalize(scrap_dir);
    let b = normalize(path);
    b.starts_with(&a)
}

fn normalize(p: &Path) -> PathBuf {
    // 只做绝对化与分隔符归一，不解析 `..`/符号链接（源码用 abspath，
    // 同样不 resolve symlink）。
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    let mut out = PathBuf::new();
    for c in p.components() {
        out.push(c.as_os_str());
    }
    out
}

/// 文件名里能解析出的编号（`re.findall(prefix + "([0-9]+)", filename)`）。
fn number_of(current: &Path, scrap_dir: &Path) -> Option<String> {
    let name = current.file_name()?.to_string_lossy();
    let dir_matches = current
        .parent()
        .map(|p| normalize(p) == normalize(scrap_dir))
        .unwrap_or(false);
    if !dir_matches {
        return None;
    }
    let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// 目录里以 `prefix + number + "_"` 开头的文件的字母后缀的最大下一字母。
fn next_revision_letter(scrap_dir: &Path, number: &str) -> Option<char> {
    let mut letter = b'a';
    let head = format!("{number}_");
    let Ok(entries) = std::fs::read_dir(scrap_dir) else {
        return Some('a');
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&head) else {
            continue;
        };
        let Some(c) = rest.chars().next() else {
            continue;
        };
        if c.is_ascii_lowercase() && c as u8 >= letter {
            letter = c as u8 + 1;
        }
    }
    if letter > b'z' {
        None
    } else {
        Some(letter as char)
    }
}

/// 目录里已有的最大三位编号（`glob(prefix + "[0-9][0-9][0-9]*")`）。
fn max_number(scrap_dir: &Path) -> u32 {
    let mut max = 0;
    let Ok(entries) = std::fs::read_dir(scrap_dir) else {
        return 0;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 3 {
            if let Ok(n) = digits.parse::<u32>() {
                max = max.max(n);
            }
        }
    }
    max
}

/// 下一次 `Save As Scrap` 的完整路径 —— `save_autoincrement_file`
/// （`filehandling.py:1362-1418`）。
///
/// `current` 是当前文档路径（可能是普通文件、已有 scrap、或 `None`）；
/// `ext` 带点（如 `".ora"`，来自 `saving.default_format`）。
///
/// `_md5...` 开头的文件直接返回自身（源码的"直接覆盖"分支）。
pub fn next_scrap_path(
    scrap_dir: &Path,
    current: Option<&Path>,
    ext: &str,
) -> PathBuf {
    if let Some(cur) = current {
        if cur
            .file_name()
            .map(|n| n.to_string_lossy().starts_with("_md5"))
            .unwrap_or(false)
        {
            return cur.to_path_buf();
        }
    }
    let number = current.and_then(|c| number_of(c, scrap_dir));
    let ext = if ext.starts_with('.') {
        ext.to_string()
    } else {
        format!(".{ext}")
    };
    let (num, letter) = match number {
        Some(n) => match next_revision_letter(scrap_dir, &n) {
            Some(c) => (n, c),
            // 字母用尽：编号 +1，回到 a（源码递归 filename=None）
            None => (format!("{:03}", max_number(scrap_dir) + 1), 'a'),
        },
        None => (format!("{:03}", max_number(scrap_dir) + 1), 'a'),
    };
    scrap_dir.join(format!("{num}_{letter}{ext}"))
}

/// 列出 scrap 目录里的文件 —— `list_scraps` / `_list_prefixed_dir`
/// （`filehandling.py:1445-1470`）。
///
/// 收：文件名以数字开头、扩展名属于 [`SCRAP_EXTENSIONS`]（大小写不敏感），
/// 或以 `_md5` 开头（特殊链接草稿纸）。按完整路径排序。
pub fn list_scraps(scrap_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(scrap_dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let Some(name) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                return false;
            };
            let numeric = name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
            if !(numeric || name.starts_with("_md5")) {
                return false;
            }
            let Some(ext) = p.extension().map(|e| e.to_string_lossy().into_owned()) else {
                return false;
            };
            SCRAP_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e))
        })
        .collect();
    out.sort();
    out
}

/// 分组 id —— `list_files_grouped.scrap_id`（`filehandling.py:1487-1492`）。
///
/// `_md5...` 文件各自成组（返回完整文件名）；其余取文件名里的第一段数字。
pub fn scrap_id(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.starts_with("_md5") {
        return name;
    }
    let digits: String = name
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        name
    } else {
        digits
    }
}

/// 按编号分组（保持排序）—— `list_scraps_grouped`（`filehandling.py:1473-1495`）。
pub fn grouped_scraps(scrap_dir: &Path) -> Vec<Vec<PathBuf>> {
    let files = list_scraps(scrap_dir);
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for f in files {
        let id = scrap_id(&f);
        match groups.last_mut() {
            Some(g) if scrap_id(&g[0]) == id => g.push(f),
            _ => groups.push(vec![f]),
        }
    }
    groups
}

/// 前后跳转的目标 —— `open_scrap_cb`（`filehandling.py:1529-1567`）。
///
/// `active` 是"最近一次打开/保存过的 scrap"；不在任何组里时，`NextScrap`
/// 从第 0 组开始、`PrevScrap` 从最后一组开始（源码的 `idx = 0` / `-1`），
/// 且都落在组内最后一个修订上。`delta` 为 `+1`/`-1`。
pub fn adjacent_scrap(
    groups: &[Vec<PathBuf>],
    active: Option<&Path>,
    delta: i32,
) -> Option<PathBuf> {
    if groups.is_empty() {
        return None;
    }
    let n = groups.len() as i32;
    let mut idx = if delta >= 0 { 0 } else { -1 };
    if let Some(active) = active {
        for (i, g) in groups.iter().enumerate() {
            if g.iter().any(|p| p == active) {
                idx = i as i32 + delta;
            }
        }
    }
    let i = idx.rem_euclid(n) as usize;
    groups.get(i).and_then(|g| g.last()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-scrap-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"x").unwrap();
        p
    }

    #[test]
    fn names_start_at_001_a_and_increment_the_letter() {
        let dir = temp_dir("letters");
        assert_eq!(
            next_scrap_path(&dir, None, ".ora"),
            dir.join("001_a.ora")
        );
        touch(&dir, "001_a.ora");
        // 当前不是 scrap（None 或普通文件）：永远开新编号（源码的
        // `number=None` → `max + 1`）
        assert_eq!(
            next_scrap_path(&dir, None, ".ora"),
            dir.join("002_a.ora")
        );
        // 当前文件是已存在的 scrap：沿用编号，找下一个字母
        let cur = dir.join("001_a.ora");
        assert_eq!(
            next_scrap_path(&dir, Some(&cur), ".ora"),
            dir.join("001_b.ora")
        );
        touch(&dir, "001_b.ora");
        assert_eq!(
            next_scrap_path(&dir, Some(&cur), ".ora"),
            dir.join("001_c.ora")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_number_when_the_letter_runs_out() {
        let dir = temp_dir("rollover");
        for c in 'a'..='z' {
            touch(&dir, &format!("001_{c}.ora"));
        }
        assert_eq!(
            next_scrap_path(&dir, None, ".ora"),
            dir.join("002_a.ora")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_comes_from_the_caller_and_md5_is_direct() {
        let dir = temp_dir("ext");
        assert_eq!(next_scrap_path(&dir, None, ".png"), dir.join("001_a.png"));
        let md5 = dir.join("_md5deadbeef.ora");
        assert_eq!(next_scrap_path(&dir, Some(&md5), ".ora"), md5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_filters_and_sorts_like_the_source() {
        let dir = temp_dir("list");
        touch(&dir, "002_a.ora");
        touch(&dir, "001_b.PNG");
        touch(&dir, "001_a.jpg");
        touch(&dir, "_md5abc.ora");
        touch(&dir, "notes.txt");
        touch(&dir, "cover.ora");
        let got: Vec<String> = list_scraps(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            got,
            ["001_a.jpg", "001_b.PNG", "002_a.ora", "_md5abc.ora"]
        );
        assert_eq!(scrap_id(Path::new("001_b.PNG")), "001");
        assert_eq!(scrap_id(Path::new("_md5abc.ora")), "_md5abc.ora");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn groups_and_adjacent_wrap_like_open_scrap_cb() {
        let dir = temp_dir("groups");
        touch(&dir, "001_a.ora");
        touch(&dir, "001_b.ora");
        touch(&dir, "002_a.ora");
        touch(&dir, "003_a.ora");
        let groups = grouped_scraps(&dir);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].len(), 2);
        // Next 从 001 组 → 002 组，落在组内最后一个修订
        let active = dir.join("001_a.ora");
        assert_eq!(
            adjacent_scrap(&groups, Some(&active), 1),
            Some(dir.join("002_a.ora"))
        );
        // Prev 从 001 组回绕到最后一组
        assert_eq!(
            adjacent_scrap(&groups, Some(&active), -1),
            Some(dir.join("003_a.ora"))
        );
        // 没有 active：Next 从 0 开始、Prev 从最后开始
        assert_eq!(
            adjacent_scrap(&groups, None, 1),
            Some(dir.join("001_b.ora"))
        );
        assert_eq!(
            adjacent_scrap(&groups, None, -1),
            Some(dir.join("003_a.ora"))
        );
        assert!(adjacent_scrap(&[], None, 1).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_user_handles_tilde_forms() {
        let home = user_home().unwrap();
        assert_eq!(expand_user("~"), home);
        assert_eq!(expand_user("~/x"), home.join("x"));
        assert_eq!(expand_user("relative"), PathBuf::from("relative"));
        let _ = home;
    }

    #[test]
    fn scrap_dir_membership_uses_absolute_paths() {
        let dir = temp_dir("inside");
        let inside = dir.join("001_a.ora");
        let outside = dir.parent().unwrap().join("elsewhere.ora");
        assert!(is_inside_scrap_dir(&dir, &inside));
        assert!(!is_inside_scrap_dir(&dir, &outside));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
