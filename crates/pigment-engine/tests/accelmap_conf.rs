//! `accelmap.conf` 的**真 GTK 门禁**（B6-2）。
//!
//! 两个 fixture 都由 `tests/tools/gen-accelmap-fixtures.py` 让**真 GTK 3.24.20**
//! 现场算出来（不是手抄、不是推导）：
//!
//! | fixture | 内容 | 生成方式 |
//! |---|---|---|
//! | `fixtures/accelmap_gtk.conf` | 整个 `accelmap.conf` | `Gtk.AccelMap.change_entry()` 灌入 125 条绑定后 `Gtk.AccelMap.save()` |
//! | `fixtures/accel_names.tsv` | 加速键字符串 → 规范名/显示名 | `Gtk.accelerator_parse/name/get_label` |
//!
//! 这两个文件把"我以为 GTK 会写/会显示什么"换成"GTK 实际写/显示了什么"。

use std::collections::BTreeSet;

use pigment_engine::accelmap::{self, format_conf, parse_conf, Accel, Entry, PROGRAM_NAME};

fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn read_fixture(name: &str) -> String {
    let path = fixture_path(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "读不到 fixture {}：{e}（重跑 tests/tools/gen-accelmap-fixtures.py）",
            path.display()
        )
    })
}

/// 把一份 conf 拆成"配置行"（去掉注释与空行、去掉行尾 `\r`）。
///
/// GTK 在 Windows 上写出 CRLF（`g_fopen` 的文本模式），这里统一剥掉，
/// 好让"行集合"的比对与宿主平台无关。
fn config_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with(';'))
        .map(str::to_string)
        .collect()
}

/// **往返门禁**：真 GTK 写出的 `accelmap.conf` → 我们的解析器 → 我们的生成器，
/// 得到的"配置行集合"必须与 GTK 的逐字节一致。
///
/// 只比行集合，不比顺序：GTK 用 `GHashTable` 的哈希序（不可依赖），
/// 我们按 path 字典序写（见 `accelmap` 模块文档）。
#[test]
fn gtk_saved_conf_round_trips_through_our_parser_and_generator() {
    let text = read_fixture("accelmap_gtk.conf");
    let parsed = parse_conf(&text);
    assert!(
        parsed.skipped.is_empty(),
        "真 GTK 写出的文件不该有解析不了的行：{:?}",
        parsed.skipped
    );
    // 105 条来自 resources.xml + 20 条来自 document.py 的槽位键
    assert_eq!(parsed.entries.len(), 125, "条目数应为 105 + 20");

    let ours = format_conf(PROGRAM_NAME, &parsed.entries);
    let ours_lines: BTreeSet<String> = config_lines(&ours).into_iter().collect();
    let gtk_lines: BTreeSet<String> = config_lines(&text).into_iter().collect();
    let only_ours: Vec<&String> = ours_lines.difference(&gtk_lines).collect();
    let only_gtk: Vec<&String> = gtk_lines.difference(&ours_lines).collect();
    assert!(
        only_ours.is_empty() && only_gtk.is_empty(),
        "我们多写的：{only_ours:?}\nGTK 有而我们没有的：{only_gtk:?}"
    );
    // 头三行同样逐字节
    assert_eq!(
        ours.lines().take(3).collect::<Vec<_>>(),
        text.lines().take(3).collect::<Vec<_>>()
    );
}

/// GTK 把"被清空的条目"写成**注释行**，解析器必须跳过它 —— 于是"删掉一个默认
/// 绑定"不会跨进程保留（下次启动默认绑定会回来）。
#[test]
fn cleared_entries_are_comments_and_never_come_back() {
    let text = read_fixture("accelmap_gtk.conf");
    let cleared = text
        .lines()
        .find(|l| l.contains("GenAccelMapFixture/ClearedEntry"))
        .expect("fixture 里应有一条被清空的条目");
    assert!(
        cleared.trim_start().starts_with(';'),
        "被清空的条目应写成注释行：{cleared:?}"
    );
    assert!(cleared.contains(r#"""#), "注释行里带空加速键：{cleared:?}");
    let parsed = parse_conf(&text);
    assert!(
        parsed
            .get("<Actions>/GenAccelMapFixture/ClearedEntry")
            .is_none(),
        "注释行不该被读成条目"
    );
}

/// **键名门禁**：源码里出现的每一条加速键字符串，我们的解析结果必须与 GTK 的
/// `accelerator_name()` / `accelerator_get_label()` 一致。
///
/// fixture 的 `INVALID` 行 = GTK 拒绝该字符串。我们只做形状校验（没有 GDK 的
/// 整张键值表），所以"拼写错误但形状合法"的三条会被收下 —— 这是**已知且记账**
/// 的差异，单列在这里，别让它悄悄变大。
#[test]
fn source_accel_specs_match_gtks_name_and_label() {
    let text = read_fixture("accel_names.tsv");
    // 形状合法、但 GTK 因"不是真键值名"而拒绝的拼写（见 accelmap 模块文档）
    const SHAPE_OK_BUT_NOT_A_GDK_NAME: &[&str] = &["Backspace", "PageUp", "Space"];
    // GTK 接受、我们**有意**拒绝的（egui 的 Modifiers 没有 Super/Meta/Hyper 位）
    const REJECTED_BY_US_ON_PURPOSE: &[&str] = &["<Super>x", "<Meta>x", "<Hyper>x"];

    let mut checked_valid = 0;
    let mut checked_invalid = 0;
    let mut exceptions = 0;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.len(),
            4,
            "fixture 行格式：spec<TAB>name<TAB>label<TAB>origin"
        );
        let (spec, want_name, want_label) = (cols[0], cols[1], cols[2]);
        let origin = cols[3];
        let parsed = Accel::parse(spec);
        if want_name == "INVALID" {
            if SHAPE_OK_BUT_NOT_A_GDK_NAME.contains(&spec) {
                let accel = parsed.unwrap_or_else(|e| {
                    panic!("{spec:?}（{origin}）是记账内的例外，应被收下：{e}")
                });
                assert_eq!(accel.key, spec);
                exceptions += 1;
                continue;
            }
            assert!(
                parsed.is_err(),
                "GTK 拒绝 {spec:?}（{origin}），我们也应拒绝，实得 {parsed:?}"
            );
            checked_invalid += 1;
            continue;
        }
        if REJECTED_BY_US_ON_PURPOSE.contains(&spec) {
            assert!(
                matches!(
                    parsed,
                    Err(pigment_engine::accelmap::AccelError::UnsupportedModifier(_))
                ),
                "GTK 收 {spec:?} 但我们有意拒绝它（egui 无对应修饰位），实得 {parsed:?}"
            );
            exceptions += 1;
            continue;
        }
        let accel = parsed.unwrap_or_else(|e| panic!("{spec:?}（{origin}）解析失败：{e}"));
        assert_eq!(
            accel.to_gtk_name(),
            want_name,
            "{spec:?}（{origin}）的规范名与 GTK 不符"
        );
        assert_eq!(
            accel.label(),
            want_label,
            "{spec:?}（{origin}）的显示名与 GTK 不符"
        );
        checked_valid += 1;
    }
    // 105 条 XML + 20 条槽位（去重后 125）+ 别名/捕获表/GTK 显示名样本
    assert_eq!(checked_valid, 164, "有效样本数（fixture 变了就同步这里）");
    assert_eq!(
        checked_invalid, 4,
        "GTK 拒绝的样本数（!、空格、空串、<Shift>）"
    );
    assert_eq!(
        exceptions,
        SHAPE_OK_BUT_NOT_A_GDK_NAME.len() + REJECTED_BY_US_ON_PURPOSE.len()
    );
}

/// fixture 里的 20 条槽位键（`gui/document.py:467-492`）必须都在，
/// 并且规范名是 `<Primary>N` / `N`。
#[test]
fn slot_keys_come_from_document_py() {
    let text = read_fixture("accelmap_gtk.conf");
    let parsed = parse_conf(&text);
    for n in 0..10 {
        let restore = parsed
            .get(&format!("<Actions>/DocumentActions/Context0{n}"))
            .unwrap_or_else(|| panic!("缺槽位恢复键 Context0{n}"));
        assert_eq!(restore.accel.as_ref().unwrap().to_gtk_name(), n.to_string());
        let store = parsed
            .get(&format!("<Actions>/DocumentActions/Context0{n}s"))
            .unwrap_or_else(|| panic!("缺槽位存入键 Context0{n}s"));
        assert_eq!(
            store.accel.as_ref().unwrap().to_gtk_name(),
            format!("<Primary>{n}"),
            "源码用小写 `<control>{n}`，规范名是 <Primary>（GTK 实测）"
        );
    }
}

/// `accelmap.conf` 与 `settings.json` 同目录（`gui/application.py:417,:503,:625`）。
#[test]
fn conf_lives_next_to_settings_json() {
    let conf = accelmap::accelmap_path();
    assert_eq!(conf.file_name().unwrap(), accelmap::FILENAME);
    assert_eq!(
        conf.parent().unwrap(),
        pigment_engine::prefs::settings_path().parent().unwrap()
    );
}

/// 空条目（`accel: None`）不参与生成器的"配置行"，但会以注释行出现 ——
/// 这条把 `Entry` 的两种"没有键"语义分开：文件里写空串 vs 注释。
#[test]
fn entries_without_a_key_are_written_as_comments() {
    let entries = vec![
        Entry::new("<Actions>/A", None),
        Entry::new("<Actions>/B", Some(Accel::parse("F5").unwrap())),
    ];
    let text = format_conf(PROGRAM_NAME, &entries);
    assert!(
        text.contains("; (gtk_accel_path \"<Actions>/A\" \"\")"),
        "{text}"
    );
    assert!(
        text.contains("(gtk_accel_path \"<Actions>/B\" \"F5\")"),
        "{text}"
    );
    assert_eq!(config_lines(&text).len(), 1, "注释掉的条目不算配置行");
}
