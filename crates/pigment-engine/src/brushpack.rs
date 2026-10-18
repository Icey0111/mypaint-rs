//! 笔刷包导入 —— `gui/brushmanager.py:647-812` 的 `import_brushpack`。
//!
//! 笔刷包是一个 zip：
//!
//! ```text
//! order.conf          # 必须存在；Group: ... + 每行一支笔刷
//! readme.txt          # 可选；有的话导入前应展示
//! group/name.myb      # 必须与 order.conf 一一对应
//! group/name_prev.png # 预览图（可选，但源码要求存在）
//! ```
//!
//! 源码逐项校验：
//! 1. 没有 `order.conf` → "not a brushpack"；
//! 2. `order.conf` 列出的笔刷必须在 zip 里有 `<name>.myb`；
//! 3. zip 里每个 `.myb` 都必须在 `order.conf` 里列出；
//! 4. 重名笔刷/分组按对话框选择覆盖或改名。本移植没有文件对话框，采用源码
//!    `window=None` 的默认语义：**不覆盖**，分组 / 笔刷都加 `#1`、`#2`…
//! 5. 把 `.myb` 与 `_prev.png` 原样写到用户笔刷目录，交给
//!    [`crate::brushlib::BrushLibrary::scan_dir`] 重新入库。

use std::collections::HashSet;
use std::io;
use std::path::Path;

use crate::zip::ZipArchive;

/// `_BRUSHPACK_ORDERCONF`（`brushmanager.py:72`）。
pub const ORDER_CONF: &str = "order.conf";
/// `_BRUSHPACK_README`（`brushmanager.py:71`）。
pub const README: &str = "readme.txt";

/// 读进来的笔刷包。
#[derive(Debug, Clone)]
pub struct Brushpack {
    pub readme: Option<String>,
    /// `order.conf` 的分组（保持文件顺序）。
    pub groups: Vec<(String, Vec<String>)>,
    /// 每支笔刷的原始字节：`(key, myb, preview)`。
    pub entries: Vec<(String, Vec<u8>, Option<Vec<u8>>)>,
}

impl Brushpack {
    /// 所有笔刷 key（含组前缀）。
    pub fn brush_keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _, _)| k.as_str())
    }

    /// 所有分组名。
    pub fn group_names(&self) -> impl Iterator<Item = &str> {
        self.groups.iter().map(|(g, _)| g.as_str())
    }
}

/// 读取并校验一个笔刷包 —— `import_brushpack` 的前半段。
pub fn read_brushpack(path: impl AsRef<Path>) -> Result<Brushpack, String> {
    let z = ZipArchive::open(path.as_ref()).map_err(|e| format!("打不开 zip：{e}"))?;
    let names = z.names();
    if !names.iter().any(|n| n == ORDER_CONF) {
        return Err(format!(
            "No file named “{ORDER_CONF}”. This is not a brushpack."
        ));
    }
    let readme = if names.iter().any(|n| n == README) {
        z.read(README).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    } else {
        None
    };
    let order = z.read(ORDER_CONF).map_err(|e| format!("读 order.conf 失败：{e}"))?;
    let groups = crate::brushlib::parse_order_conf(&String::from_utf8_lossy(&order));
    let mut listed: Vec<String> = Vec::new();
    for (_, brushes) in &groups {
        for b in brushes {
            if !listed.contains(b) {
                listed.push(b.clone());
            }
        }
    }
    // 3a. order.conf 里的笔刷必须在 zip 里
    for brush in &listed {
        let myb = format!("{brush}.myb");
        if !names.iter().any(|n| n == &myb) {
            return Err(format!(
                "Brush “{brush}” is listed in “{ORDER_CONF}”, but it does not exist in the zipfile."
            ));
        }
    }
    // 3b. zip 里的 .myb 必须在 order.conf 里
    for name in names {
        if let Some(brush) = name.strip_suffix(".myb") {
            if !listed.iter().any(|b| b == brush) {
                return Err(format!(
                    "Brush “{brush}” exists in the zipfile, but it is not listed in “{ORDER_CONF}”."
                ));
            }
        }
    }
    let mut entries = Vec::new();
    for brush in listed {
        let myb = z
            .read(&format!("{brush}.myb"))
            .map_err(|e| format!("读 {brush}.myb 失败：{e}"))?;
        let prev = z.read(&format!("{brush}_prev.png")).ok();
        entries.push((brush, myb, prev));
    }
    Ok(Brushpack {
        readme,
        groups,
        entries,
    })
}

/// 导入结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// 实际写入的分组名（可能带 `#1`）。
    pub groups: Vec<String>,
    /// 写入的笔刷 key。
    pub brushes: Vec<String>,
    /// 因重名而改名的条目（`旧 → 新`）。
    pub renames: Vec<(String, String)>,
    pub readme: Option<String>,
}

/// 唯一化：`base` 已在 `taken` 里就依次尝试 `base#1`、`base#2`…（源码
/// `DONT_OVERWRITE_THIS` 分支的命名规则）。
fn unique_name(base: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    let mut i = 1;
    loop {
        let candidate = format!("{base}#{i}");
        if !taken.contains(&candidate) {
            return candidate;
        }
        i += 1;
    }
}

/// 把已经解析好的笔刷包写进用户笔刷目录 —— `import_brushpack` 的后半段。
///
/// `existing_keys` 是库里已有的笔刷 key（含内嵌官方包）；`existing_groups`
/// 是已有分组名。不覆盖任何已有文件，冲突项按 `#1` 起改名。
pub fn install_brushpack(
    pack: &Brushpack,
    user_dir: &Path,
    existing_keys: &HashSet<String>,
    existing_groups: &HashSet<String>,
) -> io::Result<ImportReport> {
    let mut taken_keys = existing_keys.clone();
    let mut taken_groups = existing_groups.clone();
    let mut group_map: Vec<(String, String)> = Vec::new();
    for (group, _) in &pack.groups {
        let new = unique_name(group, &taken_groups);
        taken_groups.insert(new.clone());
        group_map.push((group.clone(), new));
    }
    let mut report = ImportReport {
        readme: pack.readme.clone(),
        ..Default::default()
    };
    let mut written: HashSet<String> = HashSet::new();
    for (old_key, myb, prev) in &pack.entries {
        // 组前缀替换：`oldgroup/leaf` → `newgroup/leaf`
        let (leaf, new_key) = match old_key.split_once('/') {
            Some((group, leaf)) => {
                let mapped = group_map
                    .iter()
                    .find(|(old, _)| old == group)
                    .map(|(_, new)| new.clone())
                    .unwrap_or_else(|| group.to_string());
                (leaf.to_string(), format!("{mapped}/{leaf}"))
            }
            None => (old_key.clone(), old_key.clone()),
        };
        let final_key = unique_name(&new_key, &taken_keys);
        taken_keys.insert(final_key.clone());
        if final_key != *old_key {
            report.renames.push((old_key.clone(), final_key.clone()));
        }
        let path = user_dir.join(format!("{final_key}.myb"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, myb)?;
        if let Some(prev) = prev {
            let prev_path = user_dir.join(format!("{final_key}_prev.png"));
            std::fs::write(prev_path, prev)?;
        }
        if written.insert(final_key.clone()) {
            report.brushes.push(final_key);
        }
        let _ = leaf;
    }
    for (_, new) in group_map {
        if !report.groups.contains(&new) {
            report.groups.push(new);
        }
    }
    Ok(report)
}

/// 导出一个分组为笔刷包 —— `BrushManager.export_group`
/// （`brushmanager.py:845-886`）。
///
/// zip 内是 `order.conf`（`Group: <组名>` + 每个笔刷 key）与每支笔刷的
/// `.myb`/`_prev.png`；顺序与库内成员顺序一致。
pub fn export_group(
    lib: &crate::brushlib::BrushLibrary,
    group_id: &str,
    filename: impl AsRef<Path>,
) -> io::Result<usize> {
    let title = lib
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .map(|g| g.display.clone())
        .unwrap_or_else(|| group_id.to_string());
    let mut order = format!("Group: {title}\n");
    let mut z = crate::zip::ZipWriter::create(filename.as_ref())
        .map_err(io::Error::other)?;
    let mut count = 0;
    for i in lib.group_members(group_id) {
        let entry = &lib.entries[i];
        let key = &entry.key;
        let json = entry
            .read_json()
            .map_err(|e| io::Error::other(e.to_string()))?;
        z.add_stored(&format!("{key}.myb"), json.as_bytes())
            .map_err(io::Error::other)?;
        if let Some(prev) = entry.preview_png() {
            z.add_stored(&format!("{key}_prev.png"), prev.as_ref())
                .map_err(io::Error::other)?;
        }
        order.push_str(key);
        order.push('\n');
        count += 1;
    }
    z.add_stored(ORDER_CONF, order.as_bytes())
        .map_err(io::Error::other)?;
    z.finish().map_err(io::Error::other)?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::ZipWriter;

    fn make_pack(path: &Path, order: &str, files: &[(&str, &[u8])]) {
        let mut z = ZipWriter::create(path).unwrap();
        z.add_stored(ORDER_CONF, order.as_bytes()).unwrap();
        for (name, data) in files {
            z.add_stored(name, data).unwrap();
        }
        z.finish().unwrap();
    }

    fn temp(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-brushpack-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn reads_a_valid_pack_and_rejects_a_missing_order_conf() {
        let dir = temp("read");
        let zip = dir.join("p.zip");
        make_pack(
            &zip,
            "Group: Test\nTest/one\nTest/two\n",
            &[
                ("Test/one.myb", b"{\"version\":3}"),
                ("Test/two.myb", b"{\"version\":3}"),
                ("Test/one_prev.png", b"png"),
            ],
        );
        let p = read_brushpack(&zip).unwrap();
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.brush_keys().count(), 2);
        assert!(p.entries[0].2.is_some());

        let bad = dir.join("bad.zip");
        let mut z = ZipWriter::create(&bad).unwrap();
        z.add_stored("x.myb", b"{}").unwrap();
        z.finish().unwrap();
        let err = read_brushpack(&bad).unwrap_err();
        assert!(err.contains("order.conf"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unlisted_and_missing_brushes_are_rejected() {
        let dir = temp("validate");
        let zip = dir.join("a.zip");
        make_pack(&zip, "Group: T\nT/one\n", &[("T/two.myb", b"{}")]);
        let err = read_brushpack(&zip).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");

        let zip = dir.join("b.zip");
        make_pack(
            &zip,
            "Group: T\nT/one\n",
            &[("T/one.myb", b"{}"), ("T/extra.myb", b"{}")],
        );
        let err = read_brushpack(&zip).unwrap_err();
        assert!(err.contains("not listed"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_renames_collisions_without_overwriting() {
        let dir = temp("install");
        let zip = dir.join("p.zip");
        make_pack(
            &zip,
            "Group: Test\nTest/one\n",
            &[("Test/one.myb", b"{\"version\":3}"), ("Test/one_prev.png", b"png")],
        );
        let pack = read_brushpack(&zip).unwrap();
        let user = dir.join("brushes");
        std::fs::create_dir_all(&user).unwrap();
        let existing = HashSet::from(["Test/one".to_string()]);
        let groups = HashSet::from(["Test".to_string()]);
        let r = install_brushpack(&pack, &user, &existing, &groups).unwrap();
        assert_eq!(r.groups, ["Test#1"]);
        assert_eq!(r.brushes, ["Test#1/one"]);
        assert_eq!(
            r.renames,
            vec![("Test/one".to_string(), "Test#1/one".to_string())]
        );
        assert!(user.join("Test#1/one.myb").is_file());
        assert!(user.join("Test#1/one_prev.png").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_name_walks_the_suffixes() {
        let mut taken = HashSet::from(["a".to_string(), "a#1".to_string()]);
        assert_eq!(unique_name("a", &taken), "a#2");
        taken.insert("a#2".into());
        assert_eq!(unique_name("a", &taken), "a#3");
        assert_eq!(unique_name("b", &taken), "b");
    }

    #[test]
    fn exported_group_reads_back_as_a_pack() {
        let lib = crate::brushlib::BrushLibrary::embedded();
        let n = lib.group_members("classic").len();
        let path = std::env::temp_dir().join(format!(
            "pigment-export-{}.zip",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let count = export_group(&lib, "classic", &path).unwrap();
        assert_eq!(count, n);
        let back = read_brushpack(&path).unwrap();
        assert_eq!(back.brush_keys().count(), count);
        assert_eq!(back.groups[0].0, "Classic");
        assert!(back.entries.iter().all(|(_, _, prev)| prev.is_some()));
        let _ = std::fs::remove_file(&path);
    }
}
