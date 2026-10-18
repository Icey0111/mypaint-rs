//! 国际化（最小可用面）—— `lib/i18n.py` + `gui/application.py` 的 locale 选择。
//!
//! ## 做法
//!
//! 仓库里带了 MyPaint 官方的 `po/zh_CN.po`（`assets/i18n/zh_CN.po`），启动后
//! 首次用到时解析成 `msgid → msgstr` 表。菜单/子菜单/动作 tooltip 在渲染时
//! 查表：locale = 中文时显示译文，否则回退英文原文。
//!
//! 这是**真翻译**（官方译文），但覆盖面只到"菜单树 + 动作标签/tooltip"这一层；
//! 本项目自己写的中文界面文案（偏好窗口、面板标题等）不经过 `tr()`，所以
//! 切到 English 时它们仍是中文（记账）。
//!
//! ## 偏好
//!
//! `lib.i18n.userlocale`：`""`/`auto` = 跟随系统（`LANG` 含 `zh` 用中文，
//! 否则英文）；`en`/`en_US`/`C` = 英文；`zh`/`zh_CN` = 中文。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Locale {
    System,
    English,
    Chinese,
}

static LOCALE: AtomicU8 = AtomicU8::new(0);

fn encode(locale: Locale) -> u8 {
    match locale {
        Locale::System => 0,
        Locale::English => 1,
        Locale::Chinese => 2,
    }
}

/// 按偏好值设置当前 locale（`lib.i18n.userlocale`）。
pub(crate) fn set_from_pref(value: &str) {
    let v = value.trim().to_ascii_lowercase();
    let locale = if v.is_empty() || v == "auto" || v == "system" {
        Locale::System
    } else if v.starts_with("zh") {
        Locale::Chinese
    } else {
        Locale::English
    };
    LOCALE.store(encode(locale), Ordering::Relaxed);
}

/// 当前生效的 locale（`System` 时按 `LANG` 猜）。
pub(crate) fn active() -> Locale {
    match LOCALE.load(Ordering::Relaxed) {
        2 => Locale::Chinese,
        1 => Locale::English,
        _ => {
            let lang = std::env::var("LANG").unwrap_or_default();
            if lang.to_ascii_lowercase().contains("zh") {
                Locale::Chinese
            } else {
                Locale::English
            }
        }
    }
}

/// 翻译一个 msgid；中文表里没有就回退原文。
pub(crate) fn tr(msgid: &str) -> &str {
    translate(active(), msgid)
}

fn translate(locale: Locale, msgid: &str) -> &str {
    if locale == Locale::Chinese {
        catalog().get(msgid).copied().unwrap_or(msgid)
    } else {
        msgid
    }
}

/// 解析后的官方中文表（只包含非空、非 fuzzy 的条目）。
fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static TABLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABLE.get_or_init(|| parse_po(include_str!("../assets/i18n/zh_CN.po")))
}

/// 极简 `.po` 解析：`msgid` / `msgstr` + 多行引号续行，跳过 fuzzy。
fn parse_po(text: &str) -> HashMap<&'static str, &'static str> {
    let mut out: HashMap<&'static str, &'static str> = HashMap::new();
    let mut id: Option<String> = None;
    let mut msgstr: Option<String> = None;
    let mut fuzzy = false;
    let mut target = 0u8; // 1 = msgid, 2 = msgstr

    let flush = |id: &mut Option<String>,
                 msgstr: &mut Option<String>,
                 fuzzy: &mut bool,
                 out: &mut HashMap<&'static str, &'static str>| {
        if !*fuzzy {
            if let (Some(i), Some(s)) = (id.take(), msgstr.take()) {
                if !i.is_empty() && !s.is_empty() {
                    let k: &'static str = Box::leak(i.into_boxed_str());
                    let v: &'static str = Box::leak(s.into_boxed_str());
                    out.insert(k, v);
                }
            }
        } else {
            *id = None;
            *msgstr = None;
        }
        *fuzzy = false;
    };

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            flush(&mut id, &mut msgstr, &mut fuzzy, &mut out);
            target = 0;
            continue;
        }
        if line.starts_with("#,") {
            if line.contains("fuzzy") {
                fuzzy = true;
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("msgid_plural ") {
            target = 0;
            continue;
        }
        if line.starts_with("msgctxt ") {
            // 上下文不参与 key：同一个英文词在不同上下文里的译文取最后一条
            target = 0;
            continue;
        }
        if line.starts_with("msgstr[") {
            target = 0;
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgid ") {
            flush(&mut id, &mut msgstr, &mut fuzzy, &mut out);
            id = Some(unquote(rest));
            target = 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgstr ") {
            msgstr = Some(unquote(rest));
            target = 2;
            continue;
        }
        if line.starts_with('"') {
            let piece = unquote(line);
            match target {
                1 => {
                    if let Some(s) = id.as_mut() {
                        s.push_str(&piece);
                    }
                }
                2 => {
                    if let Some(s) = msgstr.as_mut() {
                        s.push_str(&piece);
                    }
                }
                _ => {}
            }
        }
    }
    flush(&mut id, &mut msgstr, &mut fuzzy, &mut out);
    out
}

/// 去掉一层引号并还原常见转义。
fn unquote(s: &str) -> String {
    let s = s.trim();
    let inner = s
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(s);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => out.push(other),
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_catalog_parses_and_translates_known_labels() {
        let table = catalog();
        assert!(table.len() > 500, "词条太少: {}", table.len());
        assert_eq!(translate(Locale::Chinese, "Fullscreen"), "全屏");
        assert_eq!(translate(Locale::Chinese, "Brush"), "笔刷");
        // 英文 locale 永远回退原文
        assert_eq!(translate(Locale::English, "Fullscreen"), "Fullscreen");
        // 没有译文的 msgid 回退原文
        let unknown = "definitely not in the catalog";
        assert_eq!(translate(Locale::Chinese, unknown), unknown);
    }

    #[test]
    fn locale_pref_values_map_to_the_expected_locale() {
        for (value, want) in [
            ("zh_CN", Locale::Chinese),
            ("zh", Locale::Chinese),
            ("en_US", Locale::English),
            ("C", Locale::English),
        ] {
            set_from_pref(value);
            assert_eq!(active(), want, "pref {value:?}");
        }
        set_from_pref("en_US");
        assert_eq!(tr("Fullscreen"), "Fullscreen");
        set_from_pref("zh_CN");
        assert_eq!(tr("Fullscreen"), "全屏");
        set_from_pref("");
    }
}
