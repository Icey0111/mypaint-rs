//! 从安装版 MyPaint 的 `lib/config.py` + `lib/localecodes.py` 生成的
//! 语言列表（`supported_locales` 83 项；标签规则照
//! `preferenceswindow.py:434-479`：英文名 + 当地拼写）。
//! 这是**生成文件**，不要手改；重新生成见 `tools/gen_locales.py`。

/// 下拉框的一个选项（glade 的 `GtkListStore` 的一行：id + 标签）。
pub(crate) struct OptionDef {
    /// 存进偏好键的 id（`GtkComboBox` 的 `id_column=0`）。
    pub(crate) id: &'static str,
    /// 列表里显示的文字（`id_column` 的下一列）。
    pub(crate) label: &'static str,
}

pub(crate) static LOCALES: &[OptionDef] = &[
    OptionDef {
        id: "",
        label: "System Language",
    },
    OptionDef {
        id: "en_US",
        label: "English (US)",
    },
    OptionDef {
        id: "af",
        label: "Afrikaans",
    },
    OptionDef {
        id: "sq",
        label: "Albanian - (Shqip)",
    },
    OptionDef {
        id: "ar",
        label: "Arabic - (عربي)",
    },
    OptionDef {
        id: "hy",
        label: "Armenian - (Հայերեն)",
    },
    OptionDef {
        id: "as",
        label: "Assamese - (অসমীয়া)",
    },
    OptionDef {
        id: "ast",
        label: "Asturian - (Asturianu)",
    },
    OptionDef {
        id: "az",
        label: "Azerbaijani - (Azərbaycanca)",
    },
    OptionDef {
        id: "eu",
        label: "Basque - (Euskara)",
    },
    OptionDef {
        id: "be",
        label: "Belarusian - (Беларуская)",
    },
    OptionDef {
        id: "bn",
        label: "Bengali (India) - (বাংলা (ভারত))",
    },
    OptionDef {
        id: "brx",
        label: "Bodo - (बर'/बड़)",
    },
    OptionDef {
        id: "bs",
        label: "Bosnian - (Bosanski)",
    },
    OptionDef {
        id: "br",
        label: "Breton - (Brezhoneg)",
    },
    OptionDef {
        id: "bg",
        label: "Bulgarian - (Български)",
    },
    OptionDef {
        id: "ca",
        label: "Catalan - (Català)",
    },
    OptionDef {
        id: "zh_CN",
        label: "Chinese (Simplified) - (中文 (简体))",
    },
    OptionDef {
        id: "zh_TW",
        label: "Chinese (Traditional) - (正體中文 (繁體))",
    },
    OptionDef {
        id: "zh_HK",
        label: "Chinese - Hong Kong (Traditional)",
    },
    OptionDef {
        id: "hr",
        label: "Croatian - (Hrvatski)",
    },
    OptionDef {
        id: "cs",
        label: "Czech - (Čeština)",
    },
    OptionDef {
        id: "da",
        label: "Danish - (Dansk)",
    },
    OptionDef {
        id: "nl",
        label: "Dutch - (Nederlands)",
    },
    OptionDef {
        id: "dz",
        label: "Dzongkha - (རྫོང་ཁ་)",
    },
    OptionDef {
        id: "en_GB",
        label: "English (British)",
    },
    OptionDef {
        id: "en_CA",
        label: "English (Canadian)",
    },
    OptionDef {
        id: "eo",
        label: "Esperanto",
    },
    OptionDef {
        id: "et",
        label: "Estonian - (Eesti keel)",
    },
    OptionDef {
        id: "fi",
        label: "Finnish - (suomi)",
    },
    OptionDef {
        id: "fr",
        label: "French - (Français)",
    },
    OptionDef {
        id: "gl",
        label: "Galician - (Galego)",
    },
    OptionDef {
        id: "ka",
        label: "Georgian - (ქართული ენა)",
    },
    OptionDef {
        id: "de",
        label: "German - (Deutsch)",
    },
    OptionDef {
        id: "el",
        label: "Greek - (Ελληνικά)",
    },
    OptionDef {
        id: "gu",
        label: "Gujarati (India) - (ગુજરાતી (ભારત))",
    },
    OptionDef {
        id: "he",
        label: "Hebrew - (עברית)",
    },
    OptionDef {
        id: "hi",
        label: "Hindi (India) - (हिन्दी (भारत))",
    },
    OptionDef {
        id: "hu",
        label: "Hungarian - (magyar)",
    },
    OptionDef {
        id: "is",
        label: "Icelandic - (íslenska)",
    },
    OptionDef {
        id: "id",
        label: "Indonesian - (Bahasa Indonesia)",
    },
    OptionDef {
        id: "ga",
        label: "Irish - (Gaeilge)",
    },
    OptionDef {
        id: "it",
        label: "Italian - (Italiano)",
    },
    OptionDef {
        id: "ja",
        label: "Japanese - (日本語)",
    },
    OptionDef {
        id: "kab",
        label: "Kabyle",
    },
    OptionDef {
        id: "kn",
        label: "Kannada - (ಕನ್ನಡ)",
    },
    OptionDef {
        id: "csb",
        label: "Kashubian - (Kaszëbsczi jãzëk)",
    },
    OptionDef {
        id: "kk",
        label: "Kazakh - (Қазақ)",
    },
    OptionDef {
        id: "ko",
        label: "Korean - (한국어)",
    },
    OptionDef {
        id: "ckb",
        label: "Kurdî - (کوردی)",
    },
    OptionDef {
        id: "lv",
        label: "Latvian - (Latviešu)",
    },
    OptionDef {
        id: "lt",
        label: "Lithuanian - (lietuvių kalba)",
    },
    OptionDef {
        id: "mai",
        label: "Maithili - (मैथिली মৈথিলী)",
    },
    OptionDef {
        id: "ms",
        label: "Malay - (Melayu)",
    },
    OptionDef {
        id: "ml",
        label: "Malayalam - (മലയാളം)",
    },
    OptionDef {
        id: "mr",
        label: "Marathi - (मराठी)",
    },
    OptionDef {
        id: "mn",
        label: "Mongolian - (Монгол)",
    },
    OptionDef {
        id: "se",
        label: "Northern Sami - (Davvisámegiella)",
    },
    OptionDef {
        id: "nb",
        label: "Norwegian (Bokmål) - (Norsk bokmål)",
    },
    OptionDef {
        id: "nn_NO",
        label: "Norwegian (Nynorsk) - (Norsk nynorsk)",
    },
    OptionDef {
        id: "oc",
        label: "Occitan",
    },
    OptionDef {
        id: "fa",
        label: "Persian - (فارسی)",
    },
    OptionDef {
        id: "pl",
        label: "Polish - (Polski)",
    },
    OptionDef {
        id: "pt_BR",
        label: "Portuguese (Brazilian) - (Português (do Brasil))",
    },
    OptionDef {
        id: "pt",
        label: "Portuguese (Portugal) - (Português (Europeu))",
    },
    OptionDef {
        id: "pa",
        label: "Punjabi (India) - (ਪੰਜਾਬੀ (ਭਾਰਤ))",
    },
    OptionDef {
        id: "ro",
        label: "Romanian - (Română)",
    },
    OptionDef {
        id: "ru",
        label: "Russian - (Русский)",
    },
    OptionDef {
        id: "sr",
        label: "Serbian - (Српски)",
    },
    OptionDef {
        id: "sr@latin",
        label: "Serbian (Latin) - (Srpski)",
    },
    OptionDef {
        id: "sk",
        label: "Slovak - (slovenčina)",
    },
    OptionDef {
        id: "sl",
        label: "Slovenian - (Slovenščina)",
    },
    OptionDef {
        id: "es",
        label: "Spanish - (Español)",
    },
    OptionDef {
        id: "sv",
        label: "Swedish - (Svenska)",
    },
    OptionDef {
        id: "tg",
        label: "Tajik - (забо́ни тоҷикӣ́)",
    },
    OptionDef {
        id: "ta",
        label: "Tamil - (தமிழ்)",
    },
    OptionDef {
        id: "te",
        label: "Telugu - (తెలుగు)",
    },
    OptionDef {
        id: "th",
        label: "Thai - (ไทย)",
    },
    OptionDef {
        id: "tr",
        label: "Turkish - (Türkçe)",
    },
    OptionDef {
        id: "uk",
        label: "Ukrainian - (Українська)",
    },
    OptionDef {
        id: "uz",
        label: "Uzbek - (Oʻzbek tili)",
    },
    OptionDef {
        id: "ca@valencia",
        label: "Valencian - (Valenciano)",
    },
    OptionDef {
        id: "vi",
        label: "Vietnamese - (Tiếng Việt)",
    },
    OptionDef {
        id: "wa",
        label: "Walloon - (Walon)",
    },
    OptionDef {
        id: "fy",
        label: "Western Frisian - (Westerlauwersk Frysk)",
    },
];
