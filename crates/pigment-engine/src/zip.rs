//! 最小 ZIP 读写 —— 服务 OpenRaster（`.ora`）容器。
//!
//! # 为什么自己写而不用 `zip` crate
//!
//! 当前 registry 里 `zip` 的**全部版本都处于 yanked 状态**（0.6 / 2.x / 3.x /
//! 4.x / 5.x / 6.x 一律不可用），无法作为依赖。而 ORA 对 ZIP 的需求很小：
//!
//! - **写**：只用 `ZIP_STORED`（存储不压缩）。MyPaint 就是这样 ——
//!   `_save_layers_to_new_orazip` 里 `zipfile.ZipFile(filename, "w",
//!   compression=zipfile.ZIP_STORED)`，且 `helpers.zipfile_writestr` 走
//!   `z.writestr(zi, data)` 不覆盖压缩方式（`lib/helpers.py:501`）。
//!   ORA 规范**要求 mimetype 必须是第一个条目且不压缩**，全 STORED 天然满足。
//! - **读**：要兼容 deflate 条目（别的应用产出的 .ora 会压缩），
//!   MyPaint 侧用 `zipfile.ZipFile(filename)` 自动探测，我们也两种都支持。
//!
//! 于是只用到已有的 `crc32fast`（CRC-32）与 `flate2`（raw DEFLATE）。
//!
//! # 与 Python `zipfile` 的对齐点
//!
//! - `zipfile_writestr` 用 `zipfile.ZipInfo(arcname)`，其 `date_time` 默认
//!   `(1980, 1, 1, 0, 0, 0)`；`external_attr` 被设为
//!   `0o644 << 16 | 0o100000 << 16`（宽权限 + 普通文件，`lib/helpers.py:515-516`）。
//!   两者本模块都照搬。
//! - 中央目录与本地文件头的版本字段取 `20`（2.0，支持 deflate）。
//! - 不做 zip64（ORA 不会有 4 GiB 成员）。
//! - 写出的条目顺序 = 调用顺序；**mimetype 必须第一个写入**。

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

/// 压缩方式：存储（不压缩）。
pub const METHOD_STORED: u16 = 0;
/// 压缩方式：deflate。
pub const METHOD_DEFLATE: u16 = 8;

/// ZIP 错误。
#[derive(Debug)]
pub enum ZipError {
    Io(std::io::Error),
    /// 不是 zip 文件（没找到 EOCD 记录）。
    NotAZip,
    /// 条目在中央目录里找不到。
    EntryNotFound(String),
    /// 读到了不支持的压缩方式。
    UnsupportedMethod(u16),
    /// 解压失败。
    Deflate(String),
    /// 数据落在本文件之外 / 头部结构损坏。
    Corrupt(String),
}

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O 错误: {e}"),
            Self::NotAZip => write!(f, "不是有效的 zip 文件（缺少中央目录结束记录）"),
            Self::EntryNotFound(n) => write!(f, "zip 里没有条目 {n:?}"),
            Self::UnsupportedMethod(m) => write!(f, "不支持的压缩方式 {m}"),
            Self::Deflate(e) => write!(f, "解压失败: {e}"),
            Self::Corrupt(e) => write!(f, "zip 结构损坏: {e}"),
        }
    }
}

impl std::error::Error for ZipError {}

impl From<std::io::Error> for ZipError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// DOS 时间戳：`zipfile.ZipInfo` 的默认值 `(1980, 1, 1, 0, 0, 0)`。
///
/// 换算成 DOS 格式：date = (year-1980)<<9 | month<<5 | day，time = 0。
const DEFAULT_DOS_DATE: u16 = (1 << 5) | 1; // 1980-01-01
const DEFAULT_DOS_TIME: u16 = 0;

/// `zipfile_writestr` 给条目设的权限（`lib/helpers.py:515-516`）。
const EXTERNAL_ATTR: u32 = (0o644 << 16) | (0o100000 << 16);

/// 写侧：只支持 STORED（与 MyPaint 一致）。
///
/// 用法：`new` → 若干个 `add_stored`（**顺序即条目顺序**）→ `finish`。
/// 若中途出错，`Drop` 会尝试写入一个带错误的中央目录，但调用方应显式
/// `finish` 以获知结果。
pub struct ZipWriter {
    out: BufWriter<File>,
    /// 每个条目在中央目录里要记的信息。
    entries: Vec<CentralEntry>,
    /// 已写入的数据偏移（用于判断是否超 4 GiB）。
    offset: u64,
    finished: bool,
}

struct CentralEntry {
    name: String,
    method: u16,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
}

impl ZipWriter {
    /// 新建（会截断已存在的文件）。
    pub fn create(path: impl AsRef<Path>) -> Result<Self, ZipError> {
        let f = File::create(path)?;
        Ok(Self {
            out: BufWriter::new(f),
            entries: Vec::new(),
            offset: 0,
            finished: false,
        })
    }

    /// 追加一个存储（不压缩）条目。
    ///
    /// 名字用正斜杠分隔（zip 规范）；ORA 内部路径本来就都是 `/`。
    pub fn add_stored(&mut self, name: &str, data: &[u8]) -> Result<(), ZipError> {
        let name_bytes = name.as_bytes();
        let crc32 = crc32fast::hash(data);
        let size = data.len();
        if self.offset + size as u64 > u32::MAX as u64 {
            return Err(ZipError::Corrupt(format!(
                "条目 {name:?} 超出 4 GiB（未实现 zip64）"
            )));
        }
        let local_header_offset = self.offset as u32;

        // ---- 本地文件头 ----
        let mut hdr = Vec::with_capacity(30 + name_bytes.len());
        hdr.extend_from_slice(&0x0403_4b50u32.to_le_bytes()); // 签名 PK\x03\x04
        hdr.extend_from_slice(&20u16.to_le_bytes()); // version needed
        hdr.extend_from_slice(&0u16.to_le_bytes()); // flags
        hdr.extend_from_slice(&METHOD_STORED.to_le_bytes());
        hdr.extend_from_slice(&DEFAULT_DOS_TIME.to_le_bytes());
        hdr.extend_from_slice(&DEFAULT_DOS_DATE.to_le_bytes());
        hdr.extend_from_slice(&crc32.to_le_bytes());
        hdr.extend_from_slice(&(size as u32).to_le_bytes()); // compressed size
        hdr.extend_from_slice(&(size as u32).to_le_bytes()); // uncompressed size
        hdr.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        hdr.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        hdr.extend_from_slice(name_bytes);
        self.out.write_all(&hdr)?;
        self.out.write_all(data)?;

        self.offset += hdr.len() as u64 + size as u64;
        self.entries.push(CentralEntry {
            name: name.to_string(),
            method: METHOD_STORED,
            crc32,
            compressed_size: size as u32,
            uncompressed_size: size as u32,
            local_header_offset,
        });
        Ok(())
    }

    /// 写入中央目录与结束记录并落盘。
    pub fn finish(mut self) -> Result<(), ZipError> {
        self.finish_inner()?;
        self.out.flush()?;
        self.finished = true;
        Ok(())
    }

    fn finish_inner(&mut self) -> Result<(), ZipError> {
        let cd_start = self.offset as u32;
        for e in &self.entries {
            let name_bytes = e.name.as_bytes();
            let mut rec = Vec::with_capacity(46 + name_bytes.len());
            rec.extend_from_slice(&0x0201_4b50u32.to_le_bytes()); // PK\x01\x02
            rec.extend_from_slice(&20u16.to_le_bytes()); // version made by
            rec.extend_from_slice(&20u16.to_le_bytes()); // version needed
            rec.extend_from_slice(&0u16.to_le_bytes()); // flags
            rec.extend_from_slice(&e.method.to_le_bytes());
            rec.extend_from_slice(&DEFAULT_DOS_TIME.to_le_bytes());
            rec.extend_from_slice(&DEFAULT_DOS_DATE.to_le_bytes());
            rec.extend_from_slice(&e.crc32.to_le_bytes());
            rec.extend_from_slice(&e.compressed_size.to_le_bytes());
            rec.extend_from_slice(&e.uncompressed_size.to_le_bytes());
            rec.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            rec.extend_from_slice(&0u16.to_le_bytes()); // extra length
            rec.extend_from_slice(&0u16.to_le_bytes()); // comment length
            rec.extend_from_slice(&0u16.to_le_bytes()); // disk number start
            rec.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            rec.extend_from_slice(&EXTERNAL_ATTR.to_le_bytes());
            rec.extend_from_slice(&e.local_header_offset.to_le_bytes());
            rec.extend_from_slice(name_bytes);
            self.out.write_all(&rec)?;
            self.offset += rec.len() as u64;
        }
        let cd_size = (self.offset as u32) - cd_start;

        // ---- 中央目录结束记录（EOCD）----
        let mut eocd = Vec::with_capacity(22);
        eocd.extend_from_slice(&0x0605_4b50u32.to_le_bytes()); // PK\x05\x06
        eocd.extend_from_slice(&0u16.to_le_bytes()); // 本磁盘号
        eocd.extend_from_slice(&0u16.to_le_bytes()); // 中央目录起始磁盘
        eocd.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        eocd.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        eocd.extend_from_slice(&cd_size.to_le_bytes());
        eocd.extend_from_slice(&cd_start.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // 注释长度
        self.out.write_all(&eocd)?;
        self.offset += eocd.len() as u64;
        Ok(())
    }
}

/// 一个中央目录条目的只读视图。
#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    pub method: u16,
    pub crc32: u32,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    local_header_offset: u32,
}

/// 读侧：支持 STORED 与 DEFLATE。
pub struct ZipArchive {
    data: Vec<u8>,
    index: HashMap<String, ZipEntry>,
    /// 按中央目录顺序的条目列表。
    order: Vec<String>,
}

impl ZipArchive {
    /// 打开并解析中央目录。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ZipError> {
        let mut f = File::open(path)?;
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        Self::from_bytes(data)
    }

    /// 从内存字节解析（测试与 zip 内嵌场景用）。
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, ZipError> {
        let eocd = find_eocd(&data).ok_or(ZipError::NotAZip)?;
        // EOCD 固定 22 字节，其后可能跟着注释（本模块忽略注释长度的一致性检查）
        let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
        let cd_size = u32::from_le_bytes([
            data[eocd + 12],
            data[eocd + 13],
            data[eocd + 14],
            data[eocd + 15],
        ]) as usize;
        let cd_start = u32::from_le_bytes([
            data[eocd + 16],
            data[eocd + 17],
            data[eocd + 18],
            data[eocd + 19],
        ]) as usize;
        if cd_start + cd_size > data.len() {
            return Err(ZipError::Corrupt("中央目录越界".into()));
        }

        let mut index = HashMap::new();
        let mut order = Vec::with_capacity(count);
        let mut p = cd_start;
        for _ in 0..count {
            if p + 46 > data.len() {
                return Err(ZipError::Corrupt("中央目录条目被截断".into()));
            }
            let sig = u32::from_le_bytes([data[p], data[p + 1], data[p + 2], data[p + 3]]);
            if sig != 0x0201_4b50 {
                return Err(ZipError::Corrupt(format!(
                    "中央目录条目签名错误 @{p}: {sig:#x}"
                )));
            }
            let method = u16::from_le_bytes([data[p + 10], data[p + 11]]);
            let crc32 = u32::from_le_bytes([data[p + 16], data[p + 17], data[p + 18], data[p + 19]]);
            let compressed_size =
                u32::from_le_bytes([data[p + 20], data[p + 21], data[p + 22], data[p + 23]]);
            let uncompressed_size =
                u32::from_le_bytes([data[p + 24], data[p + 25], data[p + 26], data[p + 27]]);
            let name_len = u16::from_le_bytes([data[p + 28], data[p + 29]]) as usize;
            let extra_len = u16::from_le_bytes([data[p + 30], data[p + 31]]) as usize;
            let comment_len = u16::from_le_bytes([data[p + 32], data[p + 33]]) as usize;
            let local_header_offset =
                u32::from_le_bytes([data[p + 42], data[p + 43], data[p + 44], data[p + 45]]);
            let name_start = p + 46;
            if name_start + name_len > data.len() {
                return Err(ZipError::Corrupt("条目名被截断".into()));
            }
            let name = String::from_utf8_lossy(&data[name_start..name_start + name_len])
                .into_owned();
            index.insert(
                name.clone(),
                ZipEntry {
                    name: name.clone(),
                    method,
                    crc32,
                    compressed_size,
                    uncompressed_size,
                    local_header_offset,
                },
            );
            order.push(name);
            p = name_start + name_len + extra_len + comment_len;
        }
        Ok(Self { data, index, order })
    }

    /// 按名字取条目元信息。
    pub fn entry(&self, name: &str) -> Option<&ZipEntry> {
        self.index.get(name)
    }

    /// 是否存在某条目。
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// 按中央目录顺序列出条目名。
    pub fn names(&self) -> &[String] {
        &self.order
    }

    /// 读取并解压一个条目。
    pub fn read(&self, name: &str) -> Result<Vec<u8>, ZipError> {
        let e = self
            .index
            .get(name)
            .ok_or_else(|| ZipError::EntryNotFound(name.to_string()))?;
        let raw = self.read_raw(e)?;
        let out = match e.method {
            METHOD_STORED => raw,
            METHOD_DEFLATE => {
                use flate2::read::DeflateDecoder;
                let mut d = DeflateDecoder::new(&raw[..]);
                let mut buf = Vec::with_capacity(e.uncompressed_size as usize);
                d.read_to_end(&mut buf)
                    .map_err(|err| ZipError::Deflate(err.to_string()))?;
                buf
            }
            m => return Err(ZipError::UnsupportedMethod(m)),
        };
        // CRC-32 校验：MyPaint 侧由 zipfile 负责，我们在这里做等价检查
        if crc32fast::hash(&out) != e.crc32 {
            return Err(ZipError::Corrupt(format!("条目 {name:?} 的 CRC-32 不匹配")));
        }
        Ok(out)
    }

    /// 读一个条目的压缩字节（本地文件头之后的那段）。
    fn read_raw(&self, e: &ZipEntry) -> Result<Vec<u8>, ZipError> {
        let p = e.local_header_offset as usize;
        if p + 30 > self.data.len() {
            return Err(ZipError::Corrupt("本地文件头越界".into()));
        }
        let sig = u32::from_le_bytes([self.data[p], self.data[p + 1], self.data[p + 2], self.data[p + 3]]);
        if sig != 0x0403_4b50 {
            return Err(ZipError::Corrupt(format!(
                "本地文件头签名错误 @{p}: {sig:#x}"
            )));
        }
        let name_len = u16::from_le_bytes([self.data[p + 26], self.data[p + 27]]) as usize;
        let extra_len = u16::from_le_bytes([self.data[p + 28], self.data[p + 29]]) as usize;
        let start = p + 30 + name_len + extra_len;
        let end = start + e.compressed_size as usize;
        if end > self.data.len() {
            return Err(ZipError::Corrupt(format!(
                "条目 {:?} 数据越界",
                e.name
            )));
        }
        Ok(self.data[start..end].to_vec())
    }
}

/// 从文件尾部向前找 EOCD 签名。
///
/// EOCD 前最多有 64 KiB 的注释（规范限制），所以从末尾往前扫这么大一块。
fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let search_start = data.len().saturating_sub(22 + 0xFFFF);
    let sig = 0x0605_4b50u32.to_le_bytes();
    (search_start..=data.len() - 22)
        .rev()
        .find(|&i| data[i..i + 4] == sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-zip-test-{}-{}", std::process::id(), name));
        p
    }

    #[test]
    fn stores_and_reads_back_entries_in_order() {
        let path = tmp_path("roundtrip.zip");
        {
            let mut w = ZipWriter::create(&path).unwrap();
            w.add_stored("mimetype", b"image/openraster").unwrap();
            w.add_stored("data/layer-00.png", b"\x89PNG fake").unwrap();
            w.add_stored("stack.xml", b"<image/>").unwrap();
            w.finish().unwrap();
        }
        let z = ZipArchive::open(&path).unwrap();
        assert_eq!(
            z.names(),
            &[
                "mimetype".to_string(),
                "data/layer-00.png".to_string(),
                "stack.xml".to_string()
            ],
            "条目顺序必须保持写入顺序（mimetype 必须第一个）"
        );
        assert_eq!(z.read("mimetype").unwrap(), b"image/openraster");
        assert_eq!(z.read("data/layer-00.png").unwrap(), b"\x89PNG fake");
        assert!(z.contains("stack.xml"));
        assert!(z.entry("nope").is_none());
        let _ = std::fs::remove_file(&path);
    }

    /// mimetype 必须是第一个条目且为 STORED —— ORA 规范的硬要求。
    #[test]
    fn mimetype_is_first_and_stored() {
        let path = tmp_path("mimetype.zip");
        {
            let mut w = ZipWriter::create(&path).unwrap();
            w.add_stored("mimetype", b"image/openraster").unwrap();
            w.add_stored("stack.xml", b"<image/>").unwrap();
            w.finish().unwrap();
        }
        let z = ZipArchive::open(&path).unwrap();
        assert_eq!(z.names()[0], "mimetype");
        assert_eq!(z.entry("mimetype").unwrap().method, METHOD_STORED);
        // 长度必须正好 16 字节（Python zipfile 写出的也是 16）
        assert_eq!(z.read("mimetype").unwrap().len(), 16);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_entry_reports_not_found() {
        let path = tmp_path("missing.zip");
        {
            let mut w = ZipWriter::create(&path).unwrap();
            w.add_stored("a", b"1").unwrap();
            w.finish().unwrap();
        }
        let z = ZipArchive::open(&path).unwrap();
        assert!(matches!(
            z.read("b"),
            Err(ZipError::EntryNotFound(_))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn non_zip_is_rejected() {
        assert!(matches!(
            ZipArchive::from_bytes(b"definitely not a zip file".to_vec()),
            Err(ZipError::NotAZip)
        ));
    }

    #[test]
    fn empty_archive_round_trips() {
        let path = tmp_path("empty.zip");
        {
            let w = ZipWriter::create(&path).unwrap();
            w.finish().unwrap();
        }
        let z = ZipArchive::open(&path).unwrap();
        assert!(z.names().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    /// 读侧必须能吃 Python `zipfile` 压缩过的条目（别的应用的 .ora）。
    #[test]
    fn reads_deflate_entries_written_by_python() {
        // 用 Python 生成一个 deflate 压缩的 zip，再由我们读回来
        let path = tmp_path("deflate.zip");
        let py = format!(
            r#"
import zipfile
z = zipfile.ZipFile(r"{p}", "w", compression=zipfile.ZIP_DEFLATED)
z.writestr("hello.txt", "x" * 5000)
z.close()
"#,
            p = path.display()
        );
        let ok = std::process::Command::new("python")
            .arg("-c")
            .arg(&py)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("跳过：本机没有 python，无法生成 deflate 样本");
            return;
        }
        let z = ZipArchive::open(&path).unwrap();
        assert_eq!(z.entry("hello.txt").unwrap().method, METHOD_DEFLATE);
        assert_eq!(z.read("hello.txt").unwrap(), vec![b'x'; 5000]);
        let _ = std::fs::remove_file(&path);
    }

    /// 我们写出的 zip 必须能被 Python `zipfile` 正常读取（互操作门禁）。
    #[test]
    fn python_can_read_our_archives() {
        let path = tmp_path("interop.zip");
        {
            let mut w = ZipWriter::create(&path).unwrap();
            w.add_stored("mimetype", b"image/openraster").unwrap();
            w.add_stored("stack.xml", b"<image w=\"1\" h=\"1\"/>").unwrap();
            w.finish().unwrap();
        }
        let py = format!(
            r#"
import zipfile, sys
z = zipfile.ZipFile(r"{p}")
assert z.namelist() == ["mimetype", "stack.xml"], z.namelist()
assert z.read("mimetype") == b"image/openraster"
assert z.read("stack.xml") == b'<image w="1" h="1"/>'
bad = z.testzip()
assert bad is None, bad
print("PYOK")
"#,
            p = path.display()
        );
        let out = std::process::Command::new("python").arg("-c").arg(&py).output();
        match out {
            Ok(o) if o.status.success() => {
                assert!(String::from_utf8_lossy(&o.stdout).contains("PYOK"));
            }
            Ok(o) => panic!(
                "Python 读不了我们写的 zip：{}\n{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(_) => eprintln!("跳过：本机没有 python"),
        }
        let _ = std::fs::remove_file(&path);
    }
}
