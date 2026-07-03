use byteorder::{LittleEndian, ReadBytesExt};
use encoding_rs::SHIFT_JIS;
use ethornell_core::{EthornellError, GameRoot, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

pub const MAGIC_PACK_FILE: &[u8] = b"PackFile";
pub const MAGIC_PACK_FILE_FULL: &[u8] = b"PackFile    ";
pub const MAGIC_BURIKO_ARC20: &[u8] = b"BURIKO ARC20";
pub const MAGIC_COMPRESSED_BG: &[u8] = b"CompressedBG___";
pub const MAGIC_DSC_FORMAT: &[u8] = b"DSC FORMAT 1.00";
pub const MAGIC_COMPILED_SCRIPT: &[u8] = b"BurikoCompiledScriptVer1.00";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArchiveFormat {
    PackFile,
    BurikoArc20,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum MagicKind {
    PackFile,
    BurikoArc20,
    CompressedBg,
    DscFormat,
    BurikoCompiledScript,
    Bse,
    Ogg,
    RiffWave,
    Png,
    Jpeg,
    Bp,
    Unknown,
}

impl MagicKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::PackFile => "PackFile",
            Self::BurikoArc20 => "BURIKO ARC20",
            Self::CompressedBg => "CompressedBG___",
            Self::DscFormat => "DSC FORMAT 1.00",
            Self::BurikoCompiledScript => "BurikoCompiledScriptVer1.00",
            Self::Bse => "BSE",
            Self::Ogg => "OggS",
            Self::RiffWave => "RIFF/WAVE",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Bp => "._bp",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchiveEntry {
    pub name: String,
    pub offset: u64,
    pub packed_size: u64,
    pub unpacked_size: Option<u64>,
    pub flags: u32,
    pub method: Option<String>,
}

pub trait ArchiveReader {
    fn entries(&self) -> Result<Vec<ArchiveEntry>>;
    fn read_entry(&mut self, name: &str) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchiveIndex {
    pub path: PathBuf,
    pub format: ArchiveFormat,
    pub entries: Vec<ArchiveEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GameArchiveSet {
    pub root: PathBuf,
    pub archives: Vec<ArchiveIndex>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceEntry {
    pub archive_path: PathBuf,
    pub entry_name: String,
    pub packed_size: u64,
    pub unpacked_size: Option<u64>,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct ResourceManager {
    set: GameArchiveSet,
    by_name: BTreeMap<String, Vec<(usize, usize)>>,
}

impl ResourceManager {
    pub fn open_game(root: impl AsRef<Path>) -> Result<Self> {
        let game = GameRoot::new(root.as_ref().to_path_buf())?;
        let mut archives = Vec::new();
        for path in archive_paths(&game)? {
            archives.push(read_archive_index(&path)?);
        }
        let set = GameArchiveSet {
            root: game.path().to_path_buf(),
            archives,
        };
        let mut by_name: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
        for (archive_index, archive) in set.archives.iter().enumerate() {
            for (entry_index, entry) in archive.entries.iter().enumerate() {
                by_name
                    .entry(normalize_resource_name(&entry.name))
                    .or_default()
                    .push((archive_index, entry_index));
            }
        }
        Ok(Self { set, by_name })
    }

    pub fn archives(&self) -> &GameArchiveSet {
        &self.set
    }

    pub fn list(&self) -> Vec<ResourceEntry> {
        let mut entries = Vec::new();
        for archive in &self.set.archives {
            for entry in &archive.entries {
                entries.push(resource_entry_from_archive(archive, entry));
            }
        }
        entries
    }

    pub fn find(&self, name: &str) -> Option<ResourceEntry> {
        let key = normalize_resource_name(name);
        if let Some(matches) = self.by_name.get(&key) {
            return matches
                .first()
                .and_then(|&(archive, entry)| self.entry_at(archive, entry));
        }
        self.by_name
            .iter()
            .filter(|(candidate, _)| candidate.ends_with(&key) || candidate.contains(&key))
            .flat_map(|(_, matches)| matches.iter())
            .next()
            .and_then(|&(archive, entry)| self.entry_at(archive, entry))
    }

    pub fn find_all(&self, query: &str) -> Vec<ResourceEntry> {
        let key = normalize_resource_name(query);
        let mut out = Vec::new();
        for (candidate, matches) in &self.by_name {
            if candidate.contains(&key) {
                for &(archive, entry) in matches {
                    if let Some(entry) = self.entry_at(archive, entry) {
                        out.push(entry);
                    }
                }
            }
        }
        out
    }

    pub fn find_in_archive(&self, archive_name: &str, entry_name: &str) -> Option<ResourceEntry> {
        let archive_key = archive_name.to_ascii_lowercase();
        let entry_key = normalize_resource_name(entry_name);
        self.list().into_iter().find(|entry| {
            entry.entry_name.eq_ignore_ascii_case(entry_name)
                && entry
                    .archive_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_ascii_lowercase() == archive_key)
                    .unwrap_or(false)
                || normalize_resource_name(&entry.entry_name) == entry_key
                    && entry
                        .archive_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(|name| name.to_ascii_lowercase() == archive_key)
                        .unwrap_or(false)
        })
    }

    pub fn read_decoded_from_archive(
        &self,
        archive_name: &str,
        entry_name: &str,
    ) -> Result<Vec<u8>> {
        let entry = self
            .find_in_archive(archive_name, entry_name)
            .ok_or_else(|| {
                EthornellError::Parse(format!(
                    "resource not found in archive {archive_name}: {entry_name}"
                ))
            })?;
        self.read_by_entry_decoded(&entry)
    }

    pub fn read_raw(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self.find(name).ok_or_else(|| {
            EthornellError::Parse(format!("resource not found in game archives: {name}"))
        })?;
        self.read_by_entry_raw(&entry)
    }

    pub fn read_decoded(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self.find(name).ok_or_else(|| {
            EthornellError::Parse(format!("resource not found in game archives: {name}"))
        })?;
        self.read_by_entry_decoded(&entry)
    }

    pub fn read_by_entry_raw(&self, entry: &ResourceEntry) -> Result<Vec<u8>> {
        let archive = self.archive_index_for(&entry.archive_path)?;
        let archive_entry = archive
            .entries
            .iter()
            .find(|candidate| {
                candidate.name == entry.entry_name && candidate.packed_size == entry.packed_size
            })
            .ok_or_else(|| {
                EthornellError::Parse(format!(
                    "resource entry not found: {}:{}",
                    entry.archive_path.display(),
                    entry.entry_name
                ))
            })?;
        read_entry_raw(&archive.path, archive_entry)
    }

    pub fn read_by_entry_decoded(&self, entry: &ResourceEntry) -> Result<Vec<u8>> {
        let raw = self.read_by_entry_raw(entry)?;
        decode_payload(&entry.entry_name, &raw)
    }

    fn archive_index_for(&self, path: &Path) -> Result<&ArchiveIndex> {
        self.set
            .archives
            .iter()
            .find(|archive| archive.path == path)
            .ok_or_else(|| {
                EthornellError::Parse(format!("archive not indexed: {}", path.display()))
            })
    }

    fn entry_at(&self, archive_index: usize, entry_index: usize) -> Option<ResourceEntry> {
        let archive = self.set.archives.get(archive_index)?;
        let entry = archive.entries.get(entry_index)?;
        Some(resource_entry_from_archive(archive, entry))
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtractSummary {
    pub archives: usize,
    pub entries: usize,
    pub written: usize,
    pub decoded: usize,
    pub raw_dumped: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileProbe {
    pub path: PathBuf,
    pub size: u64,
    pub extension: Option<String>,
    pub magic: MagicKind,
    pub archive_format: ArchiveFormat,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanReport {
    pub game_root: PathBuf,
    pub file_count: usize,
    pub arc_candidates: usize,
    pub bp_scripts: usize,
    pub scenario_candidates: usize,
    pub compressed_bg_candidates: usize,
    pub ogg_candidates: usize,
    pub unknown_count: usize,
    pub files: Vec<FileProbe>,
}

pub fn detect_magic(buf: &[u8]) -> MagicKind {
    if buf.starts_with(MAGIC_BURIKO_ARC20) {
        MagicKind::BurikoArc20
    } else if buf.starts_with(MAGIC_PACK_FILE) {
        MagicKind::PackFile
    } else if buf.starts_with(MAGIC_COMPRESSED_BG) {
        MagicKind::CompressedBg
    } else if buf.starts_with(MAGIC_DSC_FORMAT) {
        MagicKind::DscFormat
    } else if buf.starts_with(MAGIC_COMPILED_SCRIPT) {
        MagicKind::BurikoCompiledScript
    } else if buf.starts_with(b"BSE 1.") {
        MagicKind::Bse
    } else if buf.starts_with(b"OggS") {
        MagicKind::Ogg
    } else if buf.len() >= 12 && buf.starts_with(b"RIFF") && &buf[8..12] == b"WAVE" {
        MagicKind::RiffWave
    } else if buf.starts_with(b"\x89PNG\r\n\x1a\n") {
        MagicKind::Png
    } else if buf.starts_with(b"\xff\xd8\xff") {
        MagicKind::Jpeg
    } else {
        MagicKind::Unknown
    }
}

pub fn detect_file_magic(path: &Path, buf: &[u8]) -> MagicKind {
    if path
        .to_string_lossy()
        .to_ascii_lowercase()
        .ends_with("._bp")
    {
        MagicKind::Bp
    } else {
        detect_magic(buf)
    }
}

pub fn detect_archive_format_from_bytes(buf: &[u8]) -> ArchiveFormat {
    match detect_magic(buf) {
        MagicKind::PackFile => ArchiveFormat::PackFile,
        MagicKind::BurikoArc20 => ArchiveFormat::BurikoArc20,
        _ => ArchiveFormat::Unknown,
    }
}

pub fn detect_archive_format(path: &Path) -> Result<ArchiveFormat> {
    let mut file = fs::File::open(path)?;
    let mut header = [0u8; 12];
    let read = file.read(&mut header)?;
    Ok(detect_archive_format_from_bytes(&header[..read]))
}

pub fn read_archive_index(path: &Path) -> Result<ArchiveIndex> {
    let mut file = fs::File::open(path)?;
    let mut magic = [0u8; 12];
    file.read_exact(&mut magic)?;
    let format = detect_archive_format_from_bytes(&magic);
    match format {
        ArchiveFormat::PackFile => read_packfile_index(path, file),
        ArchiveFormat::BurikoArc20 => read_arc20_index(path, file),
        ArchiveFormat::Unknown => Err(EthornellError::UnsupportedFormat(format!(
            "{} is not a known BGI archive",
            path.display()
        ))),
    }
}

fn read_packfile_index(path: &Path, mut file: fs::File) -> Result<ArchiveIndex> {
    let count = file.read_u32::<LittleEndian>()?;
    validate_entry_count(count)?;
    let base_offset = 0x10u64 + count as u64 * 0x20;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = read_name(&mut file, 0x10)?;
        let relative_offset = file.read_u32::<LittleEndian>()?;
        let size = file.read_u32::<LittleEndian>()?;
        let mut padding = [0u8; 8];
        file.read_exact(&mut padding)?;
        entries.push(ArchiveEntry {
            name,
            offset: base_offset + relative_offset as u64,
            packed_size: size as u64,
            unpacked_size: None,
            flags: 0,
            method: None,
        });
    }
    validate_entries(path, &entries)?;
    Ok(ArchiveIndex {
        path: path.to_path_buf(),
        format: ArchiveFormat::PackFile,
        entries,
    })
}

fn read_arc20_index(path: &Path, mut file: fs::File) -> Result<ArchiveIndex> {
    let count = file.read_u32::<LittleEndian>()?;
    validate_entry_count(count)?;
    let base_offset = 0x10u64 + count as u64 * 0x80;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = read_name(&mut file, 0x60)?;
        let relative_offset = file.read_u32::<LittleEndian>()?;
        let size = file.read_u32::<LittleEndian>()?;
        let mut tail = [0u8; 24];
        file.read_exact(&mut tail)?;
        entries.push(ArchiveEntry {
            name,
            offset: base_offset + relative_offset as u64,
            packed_size: size as u64,
            unpacked_size: None,
            flags: 0,
            method: None,
        });
    }
    validate_entries(path, &entries)?;
    Ok(ArchiveIndex {
        path: path.to_path_buf(),
        format: ArchiveFormat::BurikoArc20,
        entries,
    })
}

fn read_name(file: &mut fs::File, len: usize) -> Result<String> {
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let (decoded, _, _) = SHIFT_JIS.decode(&bytes[..end]);
    Ok(decoded.trim_end_matches('\0').to_string())
}

fn validate_entry_count(count: u32) -> Result<()> {
    if count > 0x0f_ffff {
        return Err(EthornellError::Parse(format!(
            "unreasonable archive entry count: {count}"
        )));
    }
    Ok(())
}

fn validate_entries(path: &Path, entries: &[ArchiveEntry]) -> Result<()> {
    let len = fs::metadata(path)?.len();
    for entry in entries {
        let end = entry
            .offset
            .checked_add(entry.packed_size)
            .ok_or_else(|| EthornellError::Parse(format!("entry {} overflows", entry.name)))?;
        if end > len {
            return Err(EthornellError::Parse(format!(
                "entry {} extends past archive end: {} > {}",
                entry.name, end, len
            )));
        }
    }
    Ok(())
}

pub fn extract_entry(path: &Path, entry: &ArchiveEntry) -> Result<Vec<u8>> {
    let raw = read_entry_raw(path, entry)?;
    decode_payload(&entry.name, &raw)
}

pub fn decode_payload(name: &str, raw: &[u8]) -> Result<Vec<u8>> {
    if raw.starts_with(MAGIC_DSC_FORMAT) {
        dsc_decode(&raw).map_err(|err| {
            EthornellError::UnsupportedFormat(format!("DSC decode failed for {name}: {err}"))
        })
    } else if raw.starts_with(b"BSE 1.") {
        Err(EthornellError::UnsupportedFormat(format!(
            "BSE wrapped entry is not decoded yet: {name}"
        )))
    } else {
        Ok(raw.to_vec())
    }
}

pub fn read_entry_raw(path: &Path, entry: &ArchiveEntry) -> Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(entry.offset))?;
    let mut data = vec![0u8; entry.packed_size as usize];
    file.read_exact(&mut data)?;
    Ok(data)
}

pub fn extract_archive(path: &Path, output_dir: &Path) -> Result<ExtractSummary> {
    extract_archive_with_options(path, output_dir, false)
}

pub fn extract_archive_with_options(
    path: &Path,
    output_dir: &Path,
    raw: bool,
) -> Result<ExtractSummary> {
    let index = read_archive_index(path)?;
    let mut summary = ExtractSummary {
        archives: 1,
        entries: index.entries.len(),
        ..ExtractSummary::default()
    };
    fs::create_dir_all(output_dir)?;
    for entry in &index.entries {
        match if raw {
            read_entry_raw(path, entry)
        } else {
            extract_entry(path, entry)
        } {
            Ok(data) => {
                let out_path = unique_output_path(&safe_output_path(output_dir, &entry.name)?);
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut out = fs::File::create(out_path)?;
                out.write_all(&data)?;
                summary.written += 1;
                if raw {
                    summary.raw_dumped += 1;
                } else {
                    summary.decoded += 1;
                }
            }
            Err(err) => {
                summary.skipped += 1;
                summary.errors.push(format!("{}: {err}", entry.name));
            }
        }
    }
    Ok(summary)
}

pub fn extract_game_archives(
    game: &GameRoot,
    output_dir: &Path,
    raw: bool,
) -> Result<ExtractSummary> {
    let archives = archive_paths(game)?;
    let mut total = ExtractSummary {
        archives: archives.len(),
        ..ExtractSummary::default()
    };
    fs::create_dir_all(output_dir)?;
    for archive in archives {
        let stem = archive
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("archive");
        let archive_out = safe_output_path(output_dir, stem)?;
        match extract_archive_with_options(&archive, &archive_out, raw) {
            Ok(summary) => {
                total.entries += summary.entries;
                total.written += summary.written;
                total.decoded += summary.decoded;
                total.raw_dumped += summary.raw_dumped;
                total.skipped += summary.skipped;
                total.errors.extend(summary.errors);
            }
            Err(err) => total.errors.push(format!("{}: {err}", archive.display())),
        }
    }
    Ok(total)
}

pub fn archive_paths(game: &GameRoot) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for file in walk_files(game.path())? {
        if detect_archive_format(&file)? != ArchiveFormat::Unknown {
            paths.push(file);
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn safe_output_path(output_dir: &Path, entry_name: &str) -> Result<PathBuf> {
    let mut out = output_dir.to_path_buf();
    let normalized = entry_name.replace('\\', "/");
    for part in Path::new(&normalized).components() {
        match part {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                let cleaned = clean_component(&name);
                if !cleaned.is_empty() {
                    out.push(cleaned);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(EthornellError::Parse(format!(
                    "unsafe archive entry path: {entry_name}"
                )));
            }
        }
    }
    if out == output_dir {
        out.push("_");
    }
    Ok(out)
}

fn unique_output_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("entry");
    let ext = path.extension().and_then(|s| s.to_str());
    for i in 1.. {
        let name = match ext {
            Some(ext) if !ext.is_empty() => format!("{stem}__{i:04}.{ext}"),
            _ => format!("{stem}__{i:04}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search should return")
}

fn clean_component(component: &str) -> String {
    component
        .chars()
        .map(|ch| match ch {
            '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            '/' | '\\' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect()
}

fn normalize_resource_name(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

fn resource_entry_from_archive(archive: &ArchiveIndex, entry: &ArchiveEntry) -> ResourceEntry {
    ResourceEntry {
        archive_path: archive.path.clone(),
        entry_name: entry.name.clone(),
        packed_size: entry.packed_size,
        unpacked_size: entry.unpacked_size,
        flags: entry.flags,
    }
}

pub fn scan_game_root(root: &GameRoot) -> Result<ScanReport> {
    let mut report = ScanReport {
        game_root: root.path().to_path_buf(),
        ..ScanReport::default()
    };
    for path in walk_files(root.path())? {
        let metadata = fs::metadata(&path)?;
        let mut header = [0u8; 64];
        let read = fs::File::open(&path)?.read(&mut header)?;
        let magic = detect_file_magic(&path, &header[..read]);
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase());
        let archive_format = detect_archive_format_from_bytes(&header[..read]);

        if extension.as_deref() == Some("arc") || archive_format != ArchiveFormat::Unknown {
            report.arc_candidates += 1;
        }
        if magic == MagicKind::Bp {
            report.bp_scripts += 1;
        }
        if magic == MagicKind::BurikoCompiledScript {
            report.scenario_candidates += 1;
        }
        if magic == MagicKind::CompressedBg {
            report.compressed_bg_candidates += 1;
        }
        if magic == MagicKind::Ogg || extension.as_deref() == Some("ogg") {
            report.ogg_candidates += 1;
        }

        report.files.push(FileProbe {
            path,
            size: metadata.len(),
            extension,
            magic,
            archive_format,
        });
    }
    report.file_count = report.files.len();
    report.unknown_count = report
        .files
        .iter()
        .filter(|probe| probe.magic == MagicKind::Unknown)
        .count();
    Ok(report)
}

pub fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk_files_inner(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_files_inner(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            walk_files_inner(&path, files)?;
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct DscNode {
    is_parent: bool,
    code: Option<u16>,
    left: usize,
    right: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct DscCode {
    depth: u8,
    code: u16,
}

pub fn dsc_decode(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 0x220 || !data.starts_with(MAGIC_DSC_FORMAT) {
        return Err(EthornellError::UnsupportedFormat(
            "missing DSC FORMAT 1.00 header".into(),
        ));
    }
    let mut key = u32::from_le_bytes([data[0x10], data[0x11], data[0x12], data[0x13]]);
    let magic = (u16::from_le_bytes([data[0], data[1]]) as u32) << 16;
    let output_size = u32::from_le_bytes([data[0x14], data[0x15], data[0x16], data[0x17]]) as usize;
    let dec_count = u32::from_le_bytes([data[0x18], data[0x19], data[0x1a], data[0x1b]]) as usize;
    if output_size > 512 * 1024 * 1024 {
        return Err(EthornellError::Parse(format!(
            "DSC output too large: {output_size}"
        )));
    }
    let mut codes = Vec::new();
    for i in 0..512usize {
        let depth = data[0x20 + i].wrapping_sub(update_key(&mut key, magic));
        if depth > 0 {
            codes.push(DscCode {
                depth,
                code: i as u16,
            });
        }
    }
    codes.sort();
    let nodes = create_dsc_tree(&codes);
    let mut reader = BitReader::new(&data[0x220..]);
    let mut output = vec![0u8; output_size];
    let mut dst = 0usize;

    for _ in 0..dec_count {
        let mut node_index = 0usize;
        loop {
            let bit = reader.next_bit()?;
            let node = nodes
                .get(node_index)
                .ok_or_else(|| EthornellError::Parse("DSC huffman node out of range".into()))?;
            node_index = if bit { node.right } else { node.left };
            let node = nodes
                .get(node_index)
                .ok_or_else(|| EthornellError::Parse("DSC huffman child out of range".into()))?;
            if !node.is_parent {
                let code = node
                    .code
                    .ok_or_else(|| EthornellError::Parse("DSC missing leaf code".into()))?;
                if (code >> 8) == 1 {
                    let offset = reader.next_bits(12)? as usize + 2;
                    let count = (code as usize & 0xff) + 2;
                    if offset > dst {
                        return Err(EthornellError::Parse(
                            "DSC back-reference before output start".into(),
                        ));
                    }
                    for _ in 0..count {
                        if dst >= output.len() {
                            break;
                        }
                        let value = output[dst - offset];
                        output[dst] = value;
                        dst += 1;
                    }
                } else if dst < output.len() {
                    output[dst] = code as u8;
                    dst += 1;
                }
                break;
            }
        }
        if dst >= output.len() {
            break;
        }
    }
    Ok(output)
}

fn create_dsc_tree(codes: &[DscCode]) -> Vec<DscNode> {
    let mut nodes = vec![
        DscNode {
            is_parent: false,
            code: None,
            left: 0,
            right: 0,
        };
        1024
    ];
    let mut left_index = vec![0usize; 512];
    let mut right_index = vec![0usize; 512];
    let mut next_node = 1usize;
    let mut depth_nodes = 1usize;
    let mut depth = 0u8;
    let mut left_child = true;
    let mut n = 0usize;
    while n < codes.len() {
        let target_left = left_child;
        left_child = !left_child;
        let mut depth_existing = 0usize;
        while n < codes.len() && codes[n].depth == depth {
            let index = if target_left {
                left_index[depth_existing]
            } else {
                right_index[depth_existing]
            };
            nodes[index].code = Some(codes[n].code);
            n += 1;
            depth_existing += 1;
        }
        let to_create = depth_nodes.saturating_sub(depth_existing);
        for i in 0..to_create {
            let index = if target_left {
                left_index[depth_existing + i]
            } else {
                right_index[depth_existing + i]
            };
            nodes[index].is_parent = true;
            if left_child {
                left_index[i * 2] = next_node;
                nodes[index].left = next_node;
                next_node += 1;
                left_index[i * 2 + 1] = next_node;
                nodes[index].right = next_node;
                next_node += 1;
            } else {
                right_index[i * 2] = next_node;
                nodes[index].left = next_node;
                next_node += 1;
                right_index[i * 2 + 1] = next_node;
                nodes[index].right = next_node;
                next_node += 1;
            }
        }
        depth = depth.saturating_add(1);
        depth_nodes = to_create * 2;
    }
    nodes
}

fn update_key(key: &mut u32, magic: u32) -> u8 {
    let v0 = 20021u32.wrapping_mul(*key & 0xffff);
    let mut v1 = magic | (*key >> 16);
    v1 = v1
        .wrapping_mul(20021)
        .wrapping_add((*key).wrapping_mul(346));
    v1 = v1.wrapping_add(v0 >> 16) & 0xffff;
    *key = (v1 << 16).wrapping_add(v0 & 0xffff).wrapping_add(1);
    v1 as u8
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bits: u32,
    nbits: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bits: 0,
            nbits: 0,
        }
    }

    fn next_bit(&mut self) -> Result<bool> {
        if self.nbits == 0 {
            let byte = self
                .data
                .get(self.pos)
                .ok_or_else(|| EthornellError::Parse("DSC bitstream exhausted".into()))?;
            self.bits = *byte as u32;
            self.pos += 1;
            self.nbits = 8;
        }
        let bit = (self.bits & 0x80) != 0;
        self.bits = (self.bits << 1) & 0xff;
        self.nbits -= 1;
        Ok(bit)
    }

    fn next_bits(&mut self, count: u8) -> Result<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            value = (value << 1) | u32::from(self.next_bit()?);
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_magic() {
        assert_eq!(detect_magic(b"BURIKO ARC20xxxx"), MagicKind::BurikoArc20);
        assert_eq!(
            detect_magic(b"CompressedBG___xxxx"),
            MagicKind::CompressedBg
        );
        assert_eq!(
            detect_archive_format_from_bytes(b"PackFile    "),
            ArchiveFormat::PackFile
        );
    }

    #[test]
    fn unknown_magic_is_stable() {
        assert_eq!(detect_magic(b"not bgi"), MagicKind::Unknown);
    }

    #[test]
    fn safe_path_rejects_traversal() {
        assert!(safe_output_path(Path::new("out"), "../bad").is_err());
        assert!(safe_output_path(Path::new("out"), "/bad").is_err());
        assert_eq!(
            safe_output_path(Path::new("out"), "dir\\a:b?.txt").unwrap(),
            PathBuf::from("out/dir/a_b_.txt")
        );
    }

    #[test]
    fn unique_path_keeps_first_name_when_free() {
        assert_eq!(
            unique_output_path(Path::new("out/free-name")),
            PathBuf::from("out/free-name")
        );
    }
}
