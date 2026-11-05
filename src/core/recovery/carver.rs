use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Sender;
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, SetFilePointerEx, FILE_BEGIN, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_SEQUENTIAL_SCAN,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use crate::common::error::Result;
use crate::core::recovery::trim_guard::TrimGuard;
use crate::hal::buffer::AlignedBuffer;
use crate::hal::windows::open_direct_io_handle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedFileType {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Pdf,
    Zip,
    SevenZip,
    Rar,
    Gz,
    Mp4,
    Avi,
    Wav,
    Mkv,
    Webp,
    Mp3,
    Flac,
    Ogg,
    Psd,
    Tiff,
    Rtf,
    Tar,
    Iso,
    Sqlite,
    Exe,
    Txt,
    Pcx,
    Docx,
    Xlsx,
    Pptx,
    Bz2,
    Wim,
    Au,
    Wma,
    Mov,
    Flv,
    Mpg,
    Wmv,
    Doc,
}

impl SupportedFileType {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Pdf => "pdf",
            Self::Zip => "zip",
            Self::SevenZip => "7z",
            Self::Rar => "rar",
            Self::Gz => "gz",
            Self::Mp4 => "mp4",
            Self::Avi => "avi",
            Self::Wav => "wav",
            Self::Mkv => "mkv",
            Self::Webp => "webp",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Psd => "psd",
            Self::Tiff => "tif",
            Self::Rtf => "rtf",
            Self::Tar => "tar",
            Self::Iso => "iso",
            Self::Sqlite => "sqlite",
            Self::Exe => "exe",
            Self::Txt => "txt",
            Self::Pcx => "pcx",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
            Self::Bz2 => "bz2",
            Self::Wim => "wim",
            Self::Au => "au",
            Self::Wma => "wma",
            Self::Mov => "mov",
            Self::Flv => "flv",
            Self::Mpg => "mpg",
            Self::Wmv => "wmv",
            Self::Doc => "doc",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "gif" => Some(Self::Gif),
            "bmp" => Some(Self::Bmp),
            "pdf" => Some(Self::Pdf),
            "zip" => Some(Self::Zip),
            "7z" => Some(Self::SevenZip),
            "rar" => Some(Self::Rar),
            "gz" => Some(Self::Gz),
            "mp4" => Some(Self::Mp4),
            "avi" => Some(Self::Avi),
            "wav" => Some(Self::Wav),
            "mkv" => Some(Self::Mkv),
            "webp" => Some(Self::Webp),
            "mp3" => Some(Self::Mp3),
            "flac" => Some(Self::Flac),
            "ogg" => Some(Self::Ogg),
            "psd" => Some(Self::Psd),
            "tif" | "tiff" => Some(Self::Tiff),
            "rtf" => Some(Self::Rtf),
            "tar" => Some(Self::Tar),
            "iso" => Some(Self::Iso),
            "sqlite" | "db" => Some(Self::Sqlite),
            "exe" | "dll" | "sys" => Some(Self::Exe),
            "txt" | "log" | "rs" | "c" | "cpp" | "py" | "json" | "xml" | "md" => Some(Self::Txt),
            "pcx" => Some(Self::Pcx),
            "docx" => Some(Self::Docx),
            "xlsx" => Some(Self::Xlsx),
            "pptx" => Some(Self::Pptx),
            "bz2" => Some(Self::Bz2),
            "wim" => Some(Self::Wim),
            "au" => Some(Self::Au),
            "wma" => Some(Self::Wma),
            "mov" => Some(Self::Mov),
            "flv" => Some(Self::Flv),
            "mpg" | "mpeg" => Some(Self::Mpg),
            "wmv" => Some(Self::Wmv),
            "doc" | "xls" | "ppt" => Some(Self::Doc),
            _ => None,
        }
    }

    pub fn header(&self) -> &'static [u8] {
        match self {
            Self::Jpeg => &[0xFF, 0xD8, 0xFF],
            Self::Png => &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
            Self::Gif => b"GIF8",
            Self::Bmp => b"BM",
            Self::Pdf => b"%PDF-",
            Self::Zip => &[0x50, 0x4B, 0x03, 0x04],
            Self::SevenZip => &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C],
            Self::Rar => &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07],
            Self::Gz => &[0x1F, 0x8B, 0x08],
            Self::Mp4 => b"ftyp", // Checked at offset +4
            Self::Avi => b"RIFF", // Checked with "AVI " at offset +8
            Self::Wav => b"RIFF", // Checked with "WAVE" at offset +8
            Self::Mkv => &[0x1A, 0x45, 0xDF, 0xA3],
            Self::Webp => b"RIFF", // Checked with "WEBP" at offset +8
            Self::Mp3 => b"ID3",
            Self::Flac => b"fLaC",
            Self::Ogg => b"OggS",
            Self::Psd => b"8BPS",
            Self::Tiff => b"II*\0", // Or MM\0*
            Self::Rtf => b"{\\rtf",
            Self::Tar => b"ustar", // Checked at offset +257
            Self::Iso => b"CD001",
            Self::Sqlite => b"SQLite format 3\0",
            Self::Exe => b"MZ",
            Self::Txt => b"",
            Self::Pcx => &[0x0A],
            Self::Docx | Self::Xlsx | Self::Pptx => &[0x50, 0x4B, 0x03, 0x04],
            Self::Bz2 => b"BZh",
            Self::Wim => b"MSWIM\0\0\0",
            Self::Au => b".snd",
            Self::Wma => &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C],
            Self::Mov => b"ftyp", // Checked at offset +4
            Self::Flv => b"FLV\x01",
            Self::Mpg => &[0x00, 0x00, 0x01, 0xBA],
            Self::Wmv => &[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11, 0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C],
            Self::Doc => &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1],
        }
    }

    pub fn max_size_bytes(&self) -> usize {
        match self {
            Self::Jpeg => 50 * 1024 * 1024,
            Self::Png => 50 * 1024 * 1024,
            Self::Gif => 50 * 1024 * 1024,
            Self::Bmp => 50 * 1024 * 1024,
            Self::Pdf => 100 * 1024 * 1024,
            Self::Zip => 200 * 1024 * 1024,
            Self::SevenZip => 500 * 1024 * 1024,
            Self::Rar => 500 * 1024 * 1024,
            Self::Gz => 200 * 1024 * 1024,
            Self::Mp4 => 1024 * 1024 * 1024,  // 1 GB
            Self::Avi => 1024 * 1024 * 1024,  // 1 GB
            Self::Wav => 500 * 1024 * 1024,
            Self::Mkv => 1024 * 1024 * 1024,  // 1 GB
            Self::Webp => 50 * 1024 * 1024,
            Self::Mp3 => 50 * 1024 * 1024,
            Self::Flac => 200 * 1024 * 1024,
            Self::Ogg => 100 * 1024 * 1024,
            Self::Psd => 200 * 1024 * 1024,
            Self::Tiff => 100 * 1024 * 1024,
            Self::Rtf => 50 * 1024 * 1024,
            Self::Tar => 500 * 1024 * 1024,
            Self::Iso => 1024 * 1024 * 1024,  // 1 GB
            Self::Sqlite => 500 * 1024 * 1024,
            Self::Exe => 200 * 1024 * 1024,
            Self::Txt => 5 * 1024 * 1024,     // 5 MB
            Self::Pcx => 50 * 1024 * 1024,
            Self::Docx => 200 * 1024 * 1024,
            Self::Xlsx => 200 * 1024 * 1024,
            Self::Pptx => 200 * 1024 * 1024,
            Self::Bz2 => 200 * 1024 * 1024,
            Self::Wim => 1024 * 1024 * 1024,  // 1 GB
            Self::Au => 100 * 1024 * 1024,
            Self::Wma => 200 * 1024 * 1024,
            Self::Mov => 1024 * 1024 * 1024,  // 1 GB
            Self::Flv => 1024 * 1024 * 1024,  // 1 GB
            Self::Mpg => 1024 * 1024 * 1024,  // 1 GB
            Self::Wmv => 1024 * 1024 * 1024,  // 1 GB
            Self::Doc => 100 * 1024 * 1024,
        }
    }

    pub fn matches_at(&self, window: &[u8], index: usize) -> bool {
        match self {
            Self::Jpeg => {
                // Must start with SOI (FF D8) followed by valid initial marker: APP0..APP15 (E0..EF), DQT (DB), or COM (FE)
                if index + 4 <= window.len() && window[index] == 0xFF && window[index + 1] == 0xD8 && window[index + 2] == 0xFF {
                    let m = window[index + 3];
                    (0xE0..=0xEF).contains(&m) || m == 0xDB || m == 0xFE
                } else {
                    false
                }
            }
            Self::Bmp => {
                // Validate BMP magic "BM", reserved fields == 0, and offset to bits >= 54
                if index + 14 <= window.len() && &window[index..index + 2] == b"BM" {
                    let reserved1 = u16::from_le_bytes([window[index + 6], window[index + 7]]);
                    let reserved2 = u16::from_le_bytes([window[index + 8], window[index + 9]]);
                    let off_bits = u32::from_le_bytes(window[index + 10..index + 14].try_into().unwrap_or([0; 4]));
                    reserved1 == 0 && reserved2 == 0 && (off_bits == 54 || off_bits == 62 || off_bits == 118 || off_bits == 124 || off_bits == 138)
                } else {
                    false
                }
            }
            Self::Gz => {
                // Validate RFC 1952 header: 1F 8B 08, flags reserved bits == 0
                if index + 10 <= window.len() && window[index] == 0x1F && window[index + 1] == 0x8B && window[index + 2] == 0x08 {
                    let flags = window[index + 3];
                    // RFC 1952: bits 5, 6, 7 are reserved and must be 0
                    if flags & 0xE0 != 0 {
                        return false;
                    }
                    true
                } else {
                    false
                }
            }
            Self::Mp4 => {
                // MP4 has 4-byte box size (16..1024) followed by "ftyp"
                if index + 12 <= window.len() {
                    let box_size = u32::from_be_bytes(window[index..index + 4].try_into().unwrap_or([0; 4])) as usize;
                    &window[index + 4..index + 8] == b"ftyp" && box_size >= 16 && box_size <= 1024
                } else {
                    false
                }
            }
            Self::Avi => {
                if index + 12 <= window.len() {
                    &window[index..index + 4] == b"RIFF" && &window[index + 8..index + 12] == b"AVI "
                } else {
                    false
                }
            }
            Self::Wav => {
                if index + 12 <= window.len() {
                    &window[index..index + 4] == b"RIFF" && &window[index + 8..index + 12] == b"WAVE"
                } else {
                    false
                }
            }
            Self::Webp => {
                if index + 12 <= window.len() {
                    &window[index..index + 4] == b"RIFF" && &window[index + 8..index + 12] == b"WEBP"
                } else {
                    false
                }
            }
            Self::Mp3 => {
                index + 10 <= window.len() && &window[index..index + 3] == b"ID3"
            }
            Self::Flac => {
                index + 4 <= window.len() && &window[index..index + 4] == b"fLaC"
            }
            Self::Ogg => {
                index + 4 <= window.len() && &window[index..index + 4] == b"OggS"
            }
            Self::Psd => {
                index + 6 <= window.len() && &window[index..index + 4] == b"8BPS" && window[index + 4] == 0 && window[index + 5] == 1
            }
            Self::Tiff => {
                index + 4 <= window.len() && (&window[index..index + 4] == b"II*\0" || &window[index..index + 4] == b"MM\0*")
            }
            Self::Rtf => {
                index + 5 <= window.len() && &window[index..index + 5] == b"{\\rtf"
            }
            Self::Tar => {
                // Must align to 512-byte block and have "ustar" magic at offset 257 (POSIX or Old GNU)
                if index % 512 == 0 && index + 512 <= window.len() {
                    let magic = &window[index + 257..index + 262];
                    magic == b"ustar"
                } else {
                    false
                }
            }
            Self::Bz2 => {
                // Header: "BZh" followed by compression block level ('1'..='9')
                if index + 4 <= window.len() && &window[index..index + 3] == b"BZh" {
                    let lvl = window[index + 3];
                    (b'1'..=b'9').contains(&lvl)
                } else {
                    false
                }
            }
            Self::Wim => {
                // Header: "MSWIM\0\0\0" followed by 4-byte cbSize (0xD0)
                if index + 12 <= window.len() && &window[index..index + 8] == b"MSWIM\0\0\0" {
                    let cb_size = u32::from_le_bytes(window[index + 8..index + 12].try_into().unwrap_or([0; 4]));
                    cb_size == 0xD0 || cb_size == 208
                } else {
                    false
                }
            }
            Self::Au => {
                // Magic ".snd" (2E 73 6E 64) with big-endian data offset >= 24
                if index + 24 <= window.len() && &window[index..index + 4] == b".snd" {
                    let data_offset = u32::from_be_bytes(window[index + 4..index + 8].try_into().unwrap_or([0; 4]));
                    let encoding = u32::from_be_bytes(window[index + 12..index + 16].try_into().unwrap_or([0; 4]));
                    data_offset >= 24 && encoding >= 1 && encoding <= 27
                } else {
                    false
                }
            }
            Self::Wma | Self::Wmv => {
                // ASF GUID: 30 26 B2 75 8E 66 CF 11 A6 D9 00 AA 00 62 CE 6C
                const ASF_GUID: &[u8; 16] = &[
                    0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11,
                    0xA6, 0xD9, 0x00, 0xAA, 0x00, 0x62, 0xCE, 0x6C,
                ];
                if index + 24 <= window.len() && &window[index..index + 16] == ASF_GUID {
                    let hdr_size = u64::from_le_bytes(window[index + 16..index + 24].try_into().unwrap_or([0; 8]));
                    hdr_size >= 24 && hdr_size <= 10 * 1024 * 1024
                } else {
                    false
                }
            }
            Self::Mov => {
                // QuickTime MOV: box size followed by "ftypqt  " or "moov" or "mdat"
                if index + 12 <= window.len() {
                    let box_size = u32::from_be_bytes(window[index..index + 4].try_into().unwrap_or([0; 4])) as usize;
                    let box_type = &window[index + 4..index + 8];
                    (box_type == b"ftyp" && box_size >= 16 && box_size <= 1024)
                        || (box_type == b"moov" && box_size >= 8)
                        || (box_type == b"wide" && box_size >= 8)
                } else {
                    false
                }
            }
            Self::Flv => {
                // FLV: "FLV\x01", type flags (0x01, 0x04, 0x05), data_offset == 9, previous_tag_size_0 == 0
                if index + 13 <= window.len() && &window[index..index + 4] == b"FLV\x01" {
                    let flags = window[index + 4];
                    let data_offset = u32::from_be_bytes(window[index + 5..index + 9].try_into().unwrap_or([0; 4]));
                    let prev_tag_size_0 = u32::from_be_bytes(window[index + 9..index + 13].try_into().unwrap_or([1; 4]));
                    (flags == 1 || flags == 4 || flags == 5) && data_offset == 9 && prev_tag_size_0 == 0
                } else {
                    false
                }
            }
            Self::Mpg => {
                // MPEG Program Stream: 00 00 01 BA (MPEG-1 or MPEG-2 pack header marker)
                if index + 14 <= window.len() && &window[index..index + 4] == &[0x00, 0x00, 0x01, 0xBA] {
                    let b4 = window[index + 4];
                    // MPEG-1: (b4 & 0xF0) == 0x20; MPEG-2: (b4 & 0xC0) == 0x40
                    (b4 & 0xF0) == 0x20 || (b4 & 0xC0) == 0x40
                } else {
                    false
                }
            }
            Self::Iso => {
                index + 6 <= window.len() && &window[index..index + 5] == b"CD001"
            }
            Self::Txt => {
                // Minimum size gate: must align to 512-byte sector boundary and contain at least 64 bytes of clean text
                if index % 512 != 0 || index + 64 > window.len() {
                    return false;
                }
                let sample = &window[index..index + 64];
                if let Ok(s) = std::str::from_utf8(sample) {
                    let printable = s.chars().filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace()).count();
                    printable >= 60 && !s.contains('\0')
                } else {
                    false
                }
            }
            Self::Pcx => {
                // Must be at least 128 bytes with strict PCX header fields
                if index + 128 <= window.len() && window[index] == 0x0A {
                    let version = window[index + 1];
                    let encoding = window[index + 2];
                    let bpp = window[index + 3];
                    let xmin = u16::from_le_bytes([window[index + 4], window[index + 5]]);
                    let ymin = u16::from_le_bytes([window[index + 6], window[index + 7]]);
                    let xmax = u16::from_le_bytes([window[index + 8], window[index + 9]]);
                    let ymax = u16::from_le_bytes([window[index + 10], window[index + 11]]);
                    let planes = window[index + 65];
                    let bytes_per_line = u16::from_le_bytes([window[index + 66], window[index + 67]]);

                    version <= 5
                        && (encoding == 1 || encoding == 0)
                        && matches!(bpp, 1 | 2 | 4 | 8)
                        && xmax >= xmin
                        && ymax >= ymin
                        && (xmax - xmin) < 10000
                        && (ymax - ymin) < 10000
                        && matches!(planes, 1 | 3 | 4)
                        && bytes_per_line > 0
                        && (bytes_per_line % 2 == 0)
                } else {
                    false
                }
            }
            _ => {
                let hdr = self.header();
                index + hdr.len() <= window.len() && &window[index..index + hdr.len()] == hdr
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct CarvedFile {
    pub file_type: SupportedFileType,
    pub offset: u64,
    pub size_bytes: usize,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum CarverEvent {
    Started { total_bytes: u64 },
    Progress {
        scanned_bytes: u64,
        total_bytes: u64,
        speed_mb_s: f64,
        files_found: usize,
    },
    FileFound(CarvedFile),
    Completed { total_found: usize },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaintextGate {
    Off,
    Forensic256,
    Standard512,
    Code1024,
    Bulk4096,
}

impl PlaintextGate {
    pub fn min_bytes(&self) -> Option<usize> {
        match self {
            Self::Off => None,
            Self::Forensic256 => Some(256),
            Self::Standard512 => Some(512),
            Self::Code1024 => Some(1024),
            Self::Bulk4096 => Some(4096),
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Off => Self::Forensic256,
            Self::Forensic256 => Self::Standard512,
            Self::Standard512 => Self::Code1024,
            Self::Code1024 => Self::Bulk4096,
            Self::Bulk4096 => Self::Off,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Forensic256 => "256B (Forensic/Keys)",
            Self::Standard512 => "512B (Standard .txt)",
            Self::Code1024 => "1KB (Code/Scripts)",
            Self::Bulk4096 => "4KB (Bulk/Logs)",
        }
    }
}

pub struct SignatureCarver {
    pub target_types: Vec<SupportedFileType>,
    pub output_dir: PathBuf,
    pub max_files: usize,
    pub is_ssd: bool,
    pub plaintext_gate: PlaintextGate,
    pub claimed_tracker: Option<Arc<crate::core::recovery::metadata::ClaimedLbaTracker>>,
    pub sparse_enabled: bool,
}

impl SignatureCarver {
    pub fn new(output_dir: PathBuf, is_ssd: bool) -> Self {
        Self::new_with_options(output_dir, is_ssd, PlaintextGate::Off)
    }

    pub fn new_with_options(output_dir: PathBuf, is_ssd: bool, plaintext_gate: PlaintextGate) -> Self {
        let mut target_types = vec![
            SupportedFileType::Jpeg,
            SupportedFileType::Png,
            SupportedFileType::Gif,
            SupportedFileType::Bmp,
            SupportedFileType::Pdf,
            SupportedFileType::Zip,
            SupportedFileType::SevenZip,
            SupportedFileType::Rar,
            SupportedFileType::Gz,
            SupportedFileType::Mp4,
            SupportedFileType::Avi,
            SupportedFileType::Wav,
            SupportedFileType::Mkv,
            SupportedFileType::Webp,
            SupportedFileType::Mp3,
            SupportedFileType::Flac,
            SupportedFileType::Ogg,
            SupportedFileType::Psd,
            SupportedFileType::Tiff,
            SupportedFileType::Rtf,
            SupportedFileType::Tar,
            SupportedFileType::Iso,
            SupportedFileType::Sqlite,
            SupportedFileType::Exe,
            SupportedFileType::Pcx,
            SupportedFileType::Bz2,
            SupportedFileType::Wim,
            SupportedFileType::Au,
            SupportedFileType::Wma,
            SupportedFileType::Mov,
            SupportedFileType::Flv,
            SupportedFileType::Mpg,
            SupportedFileType::Wmv,
            SupportedFileType::Doc,
        ];
        if plaintext_gate.min_bytes().is_some() {
            target_types.push(SupportedFileType::Txt);
        }
        Self {
            target_types,
            output_dir,
            max_files: usize::MAX,
            is_ssd,
            plaintext_gate,
            claimed_tracker: None,
            sparse_enabled: false,
        }
    }

    pub fn with_claimed_tracker(mut self, tracker: Arc<crate::core::recovery::metadata::ClaimedLbaTracker>) -> Self {
        self.claimed_tracker = Some(tracker);
        self
    }

    pub fn with_sparse(mut self, sparse: bool) -> Self {
        self.sparse_enabled = sparse;
        self
    }

    /// Direct I/O carving on physical drive with Win32 FILE_FLAG_NO_BUFFERING
    pub fn carve_physical_drive(
        &self,
        disk_number: u32,
        start_offset: u64,
        total_bytes: u64,
        tx: Option<Sender<CarverEvent>>,
        pause: Option<Arc<AtomicBool>>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<CarvedFile>> {
        let handle_guard = open_direct_io_handle(disk_number, false)?;
        let handle = handle_guard.0;

        fs::create_dir_all(&self.output_dir)?;

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Started { total_bytes });
        }

        let mut carved_files = Vec::new();
        let chunk_size = 1024 * 1024; // 1 MB streaming window (256 * 4096 bytes)
        let mut buffer = AlignedBuffer::new(chunk_size, 4096);
        let mut scratch_buf = AlignedBuffer::new(64 * 1024, 4096);
        let mut current_offset = start_offset;

        let mut trim_guard = TrimGuard::new(self.is_ssd, 32);
        let mut consecutive_trim_bails = 0;

        let mut last_progress_time = Instant::now();
        let mut last_progress_bytes = 0u64;

        while current_offset < total_bytes && carved_files.len() < self.max_files {
            if let Some(ref c) = cancel {
                if c.load(Ordering::Relaxed) {
                    tracing::info!("Carver physical drive session cancelled by user.");
                    break;
                }
            }

            while let Some(ref p) = pause {
                if p.load(Ordering::Relaxed) {
                    if let Some(ref c) = cancel {
                        if c.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                } else {
                    break;
                }
            }

            let remaining = total_bytes.saturating_sub(current_offset);
            if remaining == 0 {
                break;
            }

            let to_read_unaligned = remaining.min(chunk_size as u64) as usize;
            let aligned_read_len = ((to_read_unaligned + 4095) / 4096) * 4096;

            let read_result = Self::direct_read_chunk(handle, current_offset, aligned_read_len, &mut buffer);

            let bytes_read = match read_result {
                Ok(n) => (n as usize).min(to_read_unaligned),
                Err(win_err) => {
                    tracing::warn!("Direct I/O read error {} at offset 0x{:X}; skipping 4KB", win_err, current_offset);
                    current_offset += 4096;
                    continue;
                }
            };

            if bytes_read == 0 {
                break;
            }

            let window = &buffer.as_slice()[..bytes_read];

            if trim_guard.inspect_block(window) {
                consecutive_trim_bails += 1;
                current_offset += 16 * 1024 * 1024;
                continue;
            }

            let mut i = 0;
            while i < bytes_read.saturating_sub(16) && carved_files.len() < self.max_files {
                let file_offset = current_offset + i as u64;
                let current_lba = file_offset / 512;
                if let Some(ref tracker) = self.claimed_tracker {
                    if tracker.is_claimed(current_lba) {
                        let rem = 512 - (i % 512);
                        i += rem;
                        continue;
                    }
                }

                for &ft in &self.target_types {
                    if ft.matches_at(window, i) {
                        let file_offset = current_offset + i as u64;
                        if let Some(size) = self.determine_file_size_direct(handle, file_offset, ft, total_bytes, &mut scratch_buf) {
                            let filename = format!("carved_{:010}_{}.{}", file_offset, carved_files.len() + 1, ft.extension());
                            let out_path = self.output_dir.join(filename);

                            if let Ok(extracted_size) = self.extract_file_direct(handle, file_offset, size, &out_path, &mut scratch_buf) {
                                // OOXML Auto-Detection: if carved as generic Zip, inspect inner payload to reclassify to docx/xlsx/pptx
                                let (final_type, final_path) = if ft == SupportedFileType::Zip {
                                    if let Some(ooxml) = Self::detect_ooxml_type_from_path(&out_path) {
                                        let new_filename = format!("carved_{:010}_{}.{}", file_offset, carved_files.len() + 1, ooxml.extension());
                                        let new_path = self.output_dir.join(new_filename);
                                        if fs::rename(&out_path, &new_path).is_ok() {
                                            (ooxml, new_path)
                                        } else {
                                            (ft, out_path)
                                        }
                                    } else {
                                        (ft, out_path)
                                    }
                                } else if ft == SupportedFileType::Wma || ft == SupportedFileType::Wmv {
                                    if let Some(asf_type) = Self::detect_asf_type_from_path(&out_path) {
                                        let new_filename = format!("carved_{:010}_{}.{}", file_offset, carved_files.len() + 1, asf_type.extension());
                                        let new_path = self.output_dir.join(new_filename);
                                        if fs::rename(&out_path, &new_path).is_ok() {
                                            (asf_type, new_path)
                                        } else {
                                            (ft, out_path)
                                        }
                                    } else {
                                        (ft, out_path)
                                    }
                                } else {
                                    (ft, out_path)
                                };

                                let cf = CarvedFile {
                                    file_type: final_type,
                                    offset: file_offset,
                                    size_bytes: extracted_size,
                                    output_path: final_path,
                                };
                                if let Some(ref sender) = tx {
                                    let _ = sender.send(CarverEvent::FileFound(cf.clone()));
                                }
                                carved_files.push(cf);
                                i += size.max(1);
                                break;
                            }
                        }
                    }
                }
                i += 512;
            }

            let advance = bytes_read.saturating_sub(4096).max(4096);
            current_offset += advance as u64;

            if last_progress_time.elapsed().as_millis() >= 200 {
                let elapsed_sec = last_progress_time.elapsed().as_secs_f64();
                let bytes_delta = current_offset.saturating_sub(last_progress_bytes);
                let speed_mb_s = if elapsed_sec > 0.0 { (bytes_delta as f64 / 1e6) / elapsed_sec } else { 0.0 };

                if let Some(ref sender) = tx {
                    let _ = sender.send(CarverEvent::Progress {
                        scanned_bytes: current_offset,
                        total_bytes,
                        speed_mb_s,
                        files_found: carved_files.len(),
                    });
                }

                last_progress_time = Instant::now();
                last_progress_bytes = current_offset;
            }
        }

        if consecutive_trim_bails > 0 {
            tracing::info!("TRIM Guard skipped {} blocks discarded by SSD deallocation.", consecutive_trim_bails);
        }

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Completed { total_found: carved_files.len() });
        }

        Ok(carved_files)
    }

    pub(crate) fn direct_read_chunk(handle: HANDLE, offset: u64, len: usize, buf: &mut AlignedBuffer) -> std::result::Result<u32, u32> {
        unsafe {
            let mut new_pos = 0i64;
            let seek_res = SetFilePointerEx(handle, offset as i64, &mut new_pos, FILE_BEGIN);
            if seek_res == 0 {
                return Err(GetLastError());
            }

            let to_read = len.min(buf.len()) as u32;
            let mut bytes_read = 0u32;
            let res = ReadFile(
                handle,
                buf.as_mut_ptr() as *mut _,
                to_read,
                &mut bytes_read,
                ptr::null_mut(),
            );

            if res == 0 {
                return Err(GetLastError());
            }

            Ok(bytes_read)
        }
    }

    fn read_direct_aligned_slice(
        handle: HANDLE,
        offset: u64,
        size: usize,
        scratch: &mut AlignedBuffer,
    ) -> std::result::Result<Vec<u8>, u32> {
        let aligned_offset = (offset / 4096) * 4096;
        let delta = (offset - aligned_offset) as usize;
        let needed = delta + size;
        let aligned_len = ((needed + 4095) / 4096) * 4096;

        let mut result = Vec::with_capacity(size);
        let mut cur_offset = aligned_offset;
        let mut remaining = size;
        let mut first = true;

        while remaining > 0 && cur_offset < aligned_offset + aligned_len as u64 {
            let chunk_to_read = scratch.len().min(aligned_len);
            let n = Self::direct_read_chunk(handle, cur_offset, chunk_to_read, scratch)?;
            if n == 0 {
                break;
            }

            let slice = &scratch.as_slice()[..n as usize];
            let start = if first {
                first = false;
                delta.min(slice.len())
            } else {
                0
            };

            let available = slice.len().saturating_sub(start);
            let take = remaining.min(available);
            result.extend_from_slice(&slice[start..start + take]);
            remaining -= take;
            cur_offset += n as u64;

            if (n as usize) < scratch.len() {
                break;
            }
        }

        Ok(result)
    }

    fn determine_file_size_direct(
        &self,
        handle: HANDLE,
        offset: u64,
        file_type: SupportedFileType,
        total_bytes: u64,
        scratch: &mut AlignedBuffer,
    ) -> Option<usize> {
        let max_len = file_type.max_size_bytes();
        let end_boundary = (offset + max_len as u64).min(total_bytes);

        match file_type {
            SupportedFileType::Tiff => {
                let to_read = (50 * 1024 * 1024).min(max_len).min((total_bytes - offset) as usize);
                let tiff_data = Self::read_direct_aligned_slice(handle, offset, to_read, scratch).ok()?;
                Self::parse_tiff_size_slice(&tiff_data)
            }
            SupportedFileType::Pcx => {
                let to_read = (50 * 1024 * 1024).min(max_len).min((total_bytes - offset) as usize);
                let pcx_data = Self::read_direct_aligned_slice(handle, offset, to_read, scratch).ok()?;
                Self::parse_pcx_size_slice(&pcx_data)
            }
            SupportedFileType::Jpeg => {
                Self::determine_jpeg_size_direct(handle, offset, end_boundary, scratch)
            }
            SupportedFileType::Png => {
                let iend_pattern = &[0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];
                let mut pos = offset + 8;
                while pos < end_boundary {
                    let chunk_size = 64 * 1024;
                    let to_read = ((end_boundary - pos) as usize).min(chunk_size);
                    let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                    if data.len() < 8 {
                        break;
                    }
                    for idx in 0..data.len().saturating_sub(7) {
                        if &data[idx..idx + 8] == iend_pattern {
                            return Some((pos + idx as u64 + 8 - offset) as usize);
                        }
                    }
                    pos += (data.len() - 7) as u64;
                }
                None
            }
            SupportedFileType::Gif => {
                let to_read = (10 * 1024 * 1024).min(max_len).min((total_bytes - offset) as usize);
                let gif_data = Self::read_direct_aligned_slice(handle, offset, to_read, scratch).ok()?;
                Self::parse_gif_size_slice(&gif_data)
            }
            SupportedFileType::Bmp => {
                // Header (14 bytes) + DIB Header (at least 40 bytes) = 54 bytes
                let header = Self::read_direct_aligned_slice(handle, offset, 54, scratch).ok()?;
                if header.len() >= 54 && &header[..2] == b"BM" {
                    let size = u32::from_le_bytes(header[2..6].try_into().unwrap_or([0; 4])) as usize;
                    let reserved1 = u16::from_le_bytes([header[6], header[7]]);
                    let reserved2 = u16::from_le_bytes([header[8], header[9]]);
                    let off_bits = u32::from_le_bytes(header[10..14].try_into().unwrap_or([0; 4])) as usize;
                    let bi_size = u32::from_le_bytes(header[14..18].try_into().unwrap_or([0; 4]));
                    let bi_planes = u16::from_le_bytes([header[26], header[27]]);
                    let bi_bit_count = u16::from_le_bytes([header[28], header[29]]);

                    if reserved1 == 0
                        && reserved2 == 0
                        && (off_bits == 54 || off_bits == 62 || off_bits == 118 || off_bits == 124 || off_bits == 138)
                        && (bi_size == 40 || bi_size == 108 || bi_size == 124 || bi_size == 12 || bi_size == 52 || bi_size == 56 || bi_size == 64)
                        && bi_planes == 1
                        && matches!(bi_bit_count, 1 | 4 | 8 | 16 | 24 | 32)
                        && size >= off_bits
                        && size <= max_len
                    {
                        return Some(size);
                    }
                }
                None
            }
            SupportedFileType::Pdf => {
                let mut pos = offset + 5;
                let mut last_eof = None;
                while pos < end_boundary {
                    let chunk_size = 64 * 1024;
                    let to_read = ((end_boundary - pos) as usize).min(chunk_size);
                    let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                    if data.len() < 5 {
                        break;
                    }
                    for idx in 0..data.len().saturating_sub(4) {
                        if &data[idx..idx + 5] == b"%%EOF" {
                            last_eof = Some((pos + idx as u64 + 5 - offset) as usize);
                        }
                    }
                    pos += (data.len() - 4) as u64;
                }
                last_eof
            }
            SupportedFileType::Zip | SupportedFileType::Docx | SupportedFileType::Xlsx | SupportedFileType::Pptx => {
                let eocd_pattern = &[0x50, 0x4B, 0x05, 0x06];
                let mut pos = offset + 4;
                while pos < end_boundary {
                    let chunk_size = 64 * 1024;
                    let to_read = ((end_boundary - pos) as usize).min(chunk_size);
                    let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                    if data.len() < 4 {
                        break;
                    }
                    for idx in 0..data.len().saturating_sub(3) {
                        if &data[idx..idx + 4] == eocd_pattern {
                            let mut comment_len = 0usize;
                            if idx + 22 <= data.len() {
                                comment_len = u16::from_le_bytes([data[idx + 20], data[idx + 21]]) as usize;
                            }
                            return Some((pos + idx as u64 + 22 + comment_len as u64 - offset) as usize);
                        }
                    }
                    pos += (data.len() - 3) as u64;
                }
                None
            }
            SupportedFileType::SevenZip => {
                // 32-byte header: NextHeaderOffset (8 bytes) + NextHeaderSize (8 bytes) at offset 12
                let header = Self::read_direct_aligned_slice(handle, offset, 32, scratch).ok()?;
                if header.len() >= 32 {
                    let next_offset = u64::from_le_bytes(header[12..20].try_into().unwrap_or([0; 8]));
                    let next_size = u64::from_le_bytes(header[20..28].try_into().unwrap_or([0; 8]));
                    let total = 32 + next_offset + next_size;
                    if total <= max_len as u64 {
                        return Some(total as usize);
                    }
                }
                None
            }
            SupportedFileType::Rar => {
                // RAR header: read blocks until End of Archive (0x7B) or 20 MB
                let mut pos = offset + 7;
                while pos < end_boundary {
                    let chunk_size = 64 * 1024;
                    let to_read = ((end_boundary - pos) as usize).min(chunk_size);
                    let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                    if data.len() < 7 {
                        break;
                    }
                    for idx in 0..data.len().saturating_sub(6) {
                        if data[idx + 2] == 0x7B {
                            return Some((pos + idx as u64 + 7 - offset) as usize);
                        }
                    }
                    pos += (data.len() - 6) as u64;
                }
                None
            }
            SupportedFileType::Gz => {
                // GZ: minimum 10 bytes, validate RFC 1952 header fields
                let header = Self::read_direct_aligned_slice(handle, offset, 10, scratch).ok()?;
                if header.len() >= 10 && header[0] == 0x1F && header[1] == 0x8B && header[2] == 0x08 {
                    let flags = header[3];
                    if flags & 0xE0 != 0 {
                        return None;
                    }
                    // Scan forward to detect end of member or run of zeros
                    let mut pos = offset + 10;
                    let scan_limit = (offset + max_len as u64).min(end_boundary);
                    let mut last_non_zero = pos;
                    while pos < scan_limit {
                        let chunk_size = 64 * 1024;
                        let to_read = ((scan_limit - pos) as usize).min(chunk_size);
                        let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                        if data.is_empty() {
                            break;
                        }
                        let mut zero_run = 0;
                        for (idx, &b) in data.iter().enumerate() {
                            if b == 0 {
                                zero_run += 1;
                                if zero_run >= 4096 {
                                    let end_file = (pos + idx as u64 + 1 - 4096).max(last_non_zero);
                                    return Some((end_file - offset) as usize);
                                }
                            } else {
                                zero_run = 0;
                                last_non_zero = pos + idx as u64 + 1;
                            }
                        }
                        pos += data.len() as u64;
                    }
                    if last_non_zero > offset + 10 {
                        return Some(((last_non_zero - offset) as usize).min(max_len));
                    }
                }
                None
            }
            SupportedFileType::Mp4 | SupportedFileType::Mov => {
                // Parse ISO box sizes
                let mut cur = offset;
                let mut found_moov = false;
                let mut found_mdat = false;
                let mut last_valid_end = offset;

                while cur < end_boundary {
                    let box_hdr = Self::read_direct_aligned_slice(handle, cur, 8, scratch).ok()?;
                    if box_hdr.len() < 8 {
                        break;
                    }
                    let box_size = u32::from_be_bytes(box_hdr[0..4].try_into().unwrap_or([0; 4])) as u64;
                    let box_type = &box_hdr[4..8];

                    const VALID_MP4_BOXES: &[&[u8; 4]] = &[
                        b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"meta", b"pdin", b"styp", b"sidx", b"moof", b"mvex",
                    ];
                    let valid_type = VALID_MP4_BOXES.iter().any(|b| &box_type == b);
                    if !valid_type {
                        break;
                    }

                    if box_type == b"moov" {
                        found_moov = true;
                    } else if box_type == b"mdat" {
                        found_mdat = true;
                    }

                    if box_size == 0 {
                        if found_moov && found_mdat {
                            last_valid_end = cur;
                        }
                        break;
                    }

                    let actual_size = if box_size == 1 {
                        let ext = Self::read_direct_aligned_slice(handle, cur + 8, 8, scratch).ok()?;
                        if ext.len() < 8 {
                            break;
                        }
                        u64::from_be_bytes(ext[..8].try_into().unwrap_or([0; 8]))
                    } else {
                        box_size
                    };

                    if box_type == b"moov" && actual_size >= 16 {
                        let check_len = (actual_size as usize).min(256);
                        if let Ok(moov_prev) = Self::read_direct_aligned_slice(handle, cur + 8, check_len - 8, scratch) {
                            if !moov_prev.windows(4).any(|w| w == b"trak" || w == b"mvhd") {
                                found_moov = false;
                            }
                        }
                    }

                    if actual_size < 8 || actual_size > max_len as u64 {
                        break;
                    }

                    cur += actual_size;
                    last_valid_end = cur;

                    if found_moov && found_mdat && cur >= end_boundary {
                        break;
                    }
                }

                // REQUIRE BOTH moov AND mdat - otherwise the MP4/MOV has no codec metadata and cannot play
                if found_moov && found_mdat && last_valid_end > offset {
                    Some(((last_valid_end - offset) as usize).min(max_len))
                } else {
                    None
                }
            }
            SupportedFileType::Avi | SupportedFileType::Wav | SupportedFileType::Webp => {
                // RIFF chunk length at offset +4 (32-bit LE)
                let header = Self::read_direct_aligned_slice(handle, offset, 12, scratch).ok()?;
                if header.len() >= 12 && &header[..4] == b"RIFF" {
                    let riff_len = u32::from_le_bytes(header[4..8].try_into().unwrap_or([0; 4])) as usize;
                    let total = riff_len + 8;
                    if total <= max_len && total >= 12 {
                        return Some(total);
                    }
                }
                None
            }
            SupportedFileType::Mkv => {
                // Matroska / WebM: scan forward up to 500 MB
                Some(100 * 1024 * 1024)
            }
            SupportedFileType::Mp3 => {
                let header = Self::read_direct_aligned_slice(handle, offset, 10, scratch).ok()?;
                if header.len() >= 10 && &header[..3] == b"ID3" {
                    let s0 = (header[6] & 0x7F) as usize;
                    let s1 = (header[7] & 0x7F) as usize;
                    let s2 = (header[8] & 0x7F) as usize;
                    let s3 = (header[9] & 0x7F) as usize;
                    let id3_size = (s0 << 21) | (s1 << 14) | (s2 << 7) | s3;
                    let id3_len = 10 + id3_size;

                    // Read up to 20 MB of audio stream
                    let to_read = (id3_len + 20 * 1024 * 1024).min(max_len).min((total_bytes - offset) as usize);
                    let audio_buf = Self::read_direct_aligned_slice(handle, offset, to_read, scratch).ok()?;
                    Self::parse_mp3_size_slice(&audio_buf, id3_len)
                } else {
                    None
                }
            }
            SupportedFileType::Flac => {
                Some(30 * 1024 * 1024)
            }
            SupportedFileType::Ogg => {
                Some(20 * 1024 * 1024)
            }
            SupportedFileType::Psd => {
                Some(50 * 1024 * 1024)
            }
            SupportedFileType::Rtf => {
                Some(10 * 1024 * 1024)
            }
            SupportedFileType::Tar => {
                let mut cur = offset;
                let mut consecutive_zero_blocks = 0;

                while cur + 512 <= offset + max_len as u64 && cur + 512 <= total_bytes {
                    let block = match Self::read_direct_aligned_slice(handle, cur, 512, scratch) {
                        Ok(b) if b.len() == 512 => b,
                        _ => break,
                    };

                    if block.iter().all(|&b| b == 0) {
                        consecutive_zero_blocks += 1;
                        cur += 512;
                        if consecutive_zero_blocks >= 2 {
                            return Some(((cur - offset) as usize).min(max_len));
                        }
                        continue;
                    } else {
                        consecutive_zero_blocks = 0;
                    }

                    let size_field = &block[124..136];
                    let size_str = String::from_utf8_lossy(size_field);
                    let trimmed = size_str.trim_matches(|c: char| c == '\0' || c == ' ');
                    if let Ok(file_size) = u64::from_str_radix(trimmed, 8) {
                        let blocks = (file_size + 511) / 512;
                        cur += 512 + (blocks * 512);
                    } else {
                        cur += 512;
                    }
                }
                if cur > offset {
                    Some(((cur - offset) as usize).min(max_len))
                } else {
                    Some(10 * 1024 * 1024)
                }
            }
            SupportedFileType::Bz2 => {
                let mut pos = offset + 4;
                let scan_limit = (offset + max_len as u64).min(total_bytes);
                let chunk_size = 64 * 1024;

                while pos < scan_limit {
                    let to_read = ((scan_limit - pos) as usize).min(chunk_size);
                    let data = match Self::read_direct_aligned_slice(handle, pos, to_read, scratch) {
                        Ok(d) if d.len() >= 10 => d,
                        _ => break,
                    };

                    if let Some(idx) = Self::find_bz2_eos_in_slice(&data) {
                        return Some((pos + idx as u64 - offset) as usize);
                    }
                    pos += (data.len().saturating_sub(16)).max(1) as u64;
                }
                None
            }
            SupportedFileType::Wim => {
                let header = match Self::read_direct_aligned_slice(handle, offset, 208, scratch) {
                    Ok(h) if h.len() >= 208 => h,
                    _ => return Some(50 * 1024 * 1024),
                };

                if &header[..8] == b"MSWIM\0\0\0" {
                    let lut_size = u64::from_le_bytes([header[48], header[49], header[50], header[51], header[52], header[53], header[54], 0]);
                    let lut_offset = u64::from_le_bytes(header[56..64].try_into().unwrap_or([0; 8]));
                    let total = lut_offset + lut_size;
                    if total > 0 && total <= max_len as u64 {
                        return Some(total as usize);
                    }
                }
                Some(50 * 1024 * 1024)
            }
            SupportedFileType::Au => {
                let header = Self::read_direct_aligned_slice(handle, offset, 24, scratch).ok()?;
                Self::parse_au_size_slice(&header)
            }
            SupportedFileType::Wma | SupportedFileType::Wmv => {
                let header = Self::read_direct_aligned_slice(handle, offset, 16 * 1024, scratch).ok()?;
                Self::parse_asf_size_slice(&header)
            }
            SupportedFileType::Flv => {
                let mut cur = offset + 9;
                let mut last_tag_end = offset + 9;
                while cur + 15 <= end_boundary {
                    let tag_hdr = Self::read_direct_aligned_slice(handle, cur, 15, scratch).ok()?;
                    if tag_hdr.len() < 15 {
                        break;
                    }
                    let tag_type = tag_hdr[4];
                    let data_size = ((tag_hdr[5] as usize) << 16) | ((tag_hdr[6] as usize) << 8) | (tag_hdr[7] as usize);

                    if tag_type != 8 && tag_type != 9 && tag_type != 18 {
                        break;
                    }
                    if data_size > 10 * 1024 * 1024 {
                        break;
                    }

                    let tag_total = 4 + 11 + data_size;
                    cur += tag_total as u64;
                    last_tag_end = cur;
                }
                if last_tag_end > offset + 13 {
                    Some(((last_tag_end + 4 - offset) as usize).min(max_len))
                } else {
                    None
                }
            }
            SupportedFileType::Mpg => {
                // MPEG Program Stream: scan forward for Program End Code 00 00 01 B9 or last valid pack
                let mut pos = offset + 4;
                let mut last_end = None;
                while pos < end_boundary {
                    let chunk_size = 64 * 1024;
                    let to_read = ((end_boundary - pos) as usize).min(chunk_size);
                    let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                    if data.len() < 4 {
                        break;
                    }
                    for idx in 0..data.len().saturating_sub(3) {
                        if data[idx] == 0x00 && data[idx + 1] == 0x00 && data[idx + 2] == 0x01 && data[idx + 3] == 0xB9 {
                            last_end = Some((pos + idx as u64 + 4 - offset) as usize);
                            return last_end;
                        }
                    }
                    pos += (data.len() - 3) as u64;
                }
                last_end.or(Some(50 * 1024 * 1024))
            }
            SupportedFileType::Iso => {
                Some(500 * 1024 * 1024)
            }
            SupportedFileType::Txt => {
                let mut pos = offset;
                let scan_limit = (offset + max_len as u64).min(total_bytes);
                let mut total_text_len = 0usize;

                while pos < scan_limit {
                    let chunk_size = 4096;
                    let to_read = ((scan_limit - pos) as usize).min(chunk_size);
                    let data = Self::read_direct_aligned_slice(handle, pos, to_read, scratch).ok()?;
                    if data.is_empty() {
                        break;
                    }

                    let mut valid_in_chunk = 0;
                    for &b in &data {
                        if b == 0 || (b < 0x08 && b != 0x00) || (b > 0x0E && b < 0x1F) {
                            break;
                        }
                        if (0x20..=0x7E).contains(&b) || b == b'\t' || b == b'\r' || b == b'\n' || b >= 0xC0 {
                            valid_in_chunk += 1;
                        } else {
                            break;
                        }
                    }

                    total_text_len += valid_in_chunk;
                    if valid_in_chunk < data.len() {
                        break;
                    }
                    pos += data.len() as u64;
                }

                let min_gate = self.plaintext_gate.min_bytes().unwrap_or(512);
                if total_text_len >= min_gate {
                    Some(total_text_len)
                } else {
                    None
                }
            }
            SupportedFileType::Sqlite => {
                // SQLite: page size at bytes 16..18, page count at bytes 28..32
                let header = Self::read_direct_aligned_slice(handle, offset, 32, scratch).ok()?;
                if header.len() >= 32 && &header[..16] == b"SQLite format 3\0" {
                    let mut page_size = u16::from_be_bytes([header[16], header[17]]) as u64;
                    if page_size == 1 {
                        page_size = 65536;
                    }
                    let page_count = u32::from_be_bytes(header[28..32].try_into().unwrap_or([0; 4])) as u64;
                    if page_size > 0 && page_count > 0 {
                        let total = page_size * page_count;
                        if total <= max_len as u64 {
                            return Some(total as usize);
                        }
                    }
                }
                None
            }
            SupportedFileType::Exe => {
                // PE binary: offset 0x3C points to PE header
                let dos_hdr = Self::read_direct_aligned_slice(handle, offset, 64, scratch).ok()?;
                if dos_hdr.len() >= 64 && &dos_hdr[..2] == b"MZ" {
                    let pe_offset = u32::from_le_bytes(dos_hdr[0x3C..0x40].try_into().unwrap_or([0; 4])) as u64;
                    if pe_offset < 1024 {
                        let pe_hdr = Self::read_direct_aligned_slice(handle, offset + pe_offset, 256, scratch).ok()?;
                        if pe_hdr.len() >= 24 && &pe_hdr[..4] == b"PE\0\0" {
                            let num_sections = u16::from_le_bytes([pe_hdr[6], pe_hdr[7]]) as usize;
                            let opt_hdr_size = u16::from_le_bytes([pe_hdr[20], pe_hdr[21]]) as u64;
                            let sec_table_offset = pe_offset + 24 + opt_hdr_size;
                            let sec_table = Self::read_direct_aligned_slice(handle, offset + sec_table_offset, num_sections * 40, scratch).ok()?;
                            let mut max_end = sec_table_offset + (num_sections * 40) as u64;
                            for s in 0..num_sections {
                                let b = s * 40;
                                if b + 24 <= sec_table.len() {
                                    let raw_size = u32::from_le_bytes(sec_table[b + 16..b + 20].try_into().unwrap_or([0; 4])) as u64;
                                    let raw_ptr = u32::from_le_bytes(sec_table[b + 20..b + 24].try_into().unwrap_or([0; 4])) as u64;
                                    max_end = max_end.max(raw_ptr + raw_size);
                                }
                            }
                            if max_end > 0 && max_end <= max_len as u64 {
                                return Some(max_end as usize);
                            }
                        }
                    }
                }
                None
            }
            SupportedFileType::Doc => {
                Some((10 * 1024 * 1024).min((total_bytes.saturating_sub(offset)) as usize))
            }
        }
    }

    /// Parse MPEG audio stream length starting after the ID3v2 header.
    /// Checks for valid MPEG Audio frame syncs (0xFFE0 mask) and Xing/Info embedded byte counters.
    fn parse_mp3_size_slice(data: &[u8], id3_len: usize) -> Option<usize> {
        if id3_len >= data.len() {
            return None;
        }

        // Skip null padding between ID3 tag and first MPEG frame (up to 4096 bytes)
        let mut pos = id3_len;
        while pos < data.len() && data[pos] == 0 && (pos - id3_len) < 4096 {
            pos += 1;
        }

        if pos + 4 > data.len() {
            return None;
        }

        // Must have valid MPEG Audio frame sync: 11 bits set (0xFF, 0xE0 mask)
        if data[pos] != 0xFF || (data[pos + 1] & 0xE0) != 0xE0 {
            return None;
        }

        let layer = (data[pos + 1] >> 1) & 0x03;
        if layer == 0 {
            return None;
        }

        // Check for Xing / Info header in first frame (indicates VBR/CBR encoder with embedded byte count)
        let search_window = &data[pos..(pos + 256).min(data.len())];
        if let Some(xing_idx) = search_window.windows(4).position(|w| w == b"Xing" || w == b"Info") {
            let xing_pos = pos + xing_idx;
            if xing_pos + 16 <= data.len() {
                let flags = u32::from_be_bytes(data[xing_pos + 4..xing_pos + 8].try_into().unwrap_or([0; 4]));
                let bytes_offset = if (flags & 0x01) != 0 { 12 } else { 8 };
                if (flags & 0x02) != 0 && xing_pos + bytes_offset + 4 <= data.len() {
                    let stream_bytes = u32::from_be_bytes(data[xing_pos + bytes_offset..xing_pos + bytes_offset + 4].try_into().unwrap_or([0; 4])) as usize;
                    if stream_bytes > 0 && stream_bytes < 50 * 1024 * 1024 {
                        let total = pos + stream_bytes;
                        let with_id3v1 = if total + 128 <= data.len() && &data[total..total + 3] == b"TAG" {
                            total + 128
                        } else {
                            total
                        };
                        return Some(with_id3v1);
                    }
                }
            }
        }

        // Frame walk: determine exact length by following MPEG frames
        let mut cur = pos;
        let mut frame_count = 0;
        while cur + 4 <= data.len() {
            if data[cur] != 0xFF || (data[cur + 1] & 0xE0) != 0xE0 {
                break;
            }

            let mpeg_ver = (data[cur + 1] >> 3) & 0x03;
            let layer = (data[cur + 1] >> 1) & 0x03;
            let bitrate_idx = (data[cur + 2] >> 4) as usize;
            let srate_idx = ((data[cur + 2] >> 2) & 0x03) as usize;
            let padding = ((data[cur + 2] >> 1) & 0x01) as usize;

            if bitrate_idx == 0 || bitrate_idx == 15 || srate_idx == 3 || layer == 0 {
                break;
            }

            let bitrate_kbps = match (mpeg_ver, layer) {
                (3, 1) => [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0][bitrate_idx],
                _ => [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0][bitrate_idx],
            };

            let sample_rate = match (mpeg_ver, srate_idx) {
                (3, 0) => 44100,
                (3, 1) => 48000,
                (3, 2) => 32000,
                (2, 0) => 22050,
                (2, 1) => 24000,
                (2, 2) => 16000,
                _ => 44100,
            };

            let frame_len = (144 * bitrate_kbps * 1000 / sample_rate) + padding;
            if frame_len < 4 {
                break;
            }

            cur += frame_len;
            frame_count += 1;
        }

        if frame_count >= 5 {
            let mut total = cur;
            if total + 128 <= data.len() && &data[total..total + 3] == b"TAG" {
                total += 128;
            }
            Some(total)
        } else {
            None
        }
    }

    /// Structure-aware GIF parser traversing sub-blocks to find true 0x3B trailer
    fn parse_gif_size_slice(data: &[u8]) -> Option<usize> {
        if data.len() < 13 || (&data[..6] != b"GIF87a" && &data[..6] != b"GIF89a") {
            return None;
        }

        let packed = data[10];
        let gct_flag = (packed & 0x80) != 0;
        let gct_size_exp = (packed & 0x07) as usize;
        let mut cur = 13;

        if gct_flag {
            let gct_bytes = 3 * (1 << (gct_size_exp + 1));
            cur += gct_bytes;
        }

        while cur < data.len() {
            let introducer = data[cur];
            if introducer == 0x3B {
                // True GIF Trailer reached
                return Some(cur + 1);
            } else if introducer == 0x21 {
                // Extension block
                if cur + 2 > data.len() {
                    break;
                }
                cur += 2; // Skip 0x21 and label
                // Walk sub-blocks until 0x00
                while cur < data.len() {
                    let sub_len = data[cur] as usize;
                    cur += 1;
                    if sub_len == 0 {
                        break;
                    }
                    cur += sub_len;
                }
            } else if introducer == 0x2C {
                // Image descriptor (10 bytes: 0x2C + 9 fields)
                if cur + 10 > data.len() {
                    break;
                }
                let img_packed = data[cur + 9];
                let lct_flag = (img_packed & 0x80) != 0;
                let lct_size_exp = (img_packed & 0x07) as usize;
                cur += 10;

                if lct_flag {
                    let lct_bytes = 3 * (1 << (lct_size_exp + 1));
                    cur += lct_bytes;
                }

                if cur >= data.len() {
                    break;
                }
                cur += 1; // LZW min code size

                // Walk raster data sub-blocks until 0x00
                while cur < data.len() {
                    let sub_len = data[cur] as usize;
                    cur += 1;
                    if sub_len == 0 {
                        break;
                    }
                    cur += sub_len;
                }
            } else {
                // Unknown/garbage byte
                break;
            }
        }

        None
    }

    /// Structure-aware TIFF parser inspecting IFD (Image File Directory) tags.
    /// Follows StripOffsets (273) and StripByteCounts (279) to calculate exact file footprint.
    fn parse_tiff_size_slice(data: &[u8]) -> Option<usize> {
        if data.len() < 8 {
            return None;
        }

        let is_le = if &data[..4] == b"II*\0" {
            true
        } else if &data[..4] == b"MM\0*" {
            false
        } else {
            return None;
        };

        let read_u16 = |buf: &[u8], offset: usize| -> Option<u16> {
            if offset + 2 <= buf.len() {
                let bytes = [buf[offset], buf[offset + 1]];
                Some(if is_le { u16::from_le_bytes(bytes) } else { u16::from_be_bytes(bytes) })
            } else {
                None
            }
        };

        let read_u32 = |buf: &[u8], offset: usize| -> Option<u32> {
            if offset + 4 <= buf.len() {
                let bytes = [buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]];
                Some(if is_le { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
            } else {
                None
            }
        };

        let mut ifd_offset = read_u32(data, 4)? as usize;
        let mut max_extent = ifd_offset;

        while ifd_offset > 0 && ifd_offset + 2 <= data.len() {
            let num_entries = read_u16(data, ifd_offset)? as usize;
            let ifd_len = 2 + num_entries * 12 + 4;
            if ifd_offset + ifd_len > data.len() {
                break;
            }
            max_extent = max_extent.max(ifd_offset + ifd_len);

            let mut strip_offsets: Vec<u32> = Vec::new();
            let mut strip_byte_counts: Vec<u32> = Vec::new();

            for e in 0..num_entries {
                let entry_pos = ifd_offset + 2 + e * 12;
                let tag = read_u16(data, entry_pos)?;
                let ftype = read_u16(data, entry_pos + 2)?;
                let count = read_u32(data, entry_pos + 4)? as usize;
                let val_or_ptr = read_u32(data, entry_pos + 8)?;

                let type_size = match ftype {
                    1 | 2 | 6 | 7 => 1,
                    3 | 8 => 2,
                    4 | 9 | 11 => 4,
                    5 | 10 | 12 => 8,
                    _ => 1,
                };
                let total_field_bytes = type_size * count;
                if total_field_bytes > 4 {
                    max_extent = max_extent.max(val_or_ptr as usize + total_field_bytes);
                }

                if tag == 273 {
                    // StripOffsets
                    if count == 1 {
                        strip_offsets.push(val_or_ptr);
                    } else if (val_or_ptr as usize) + count * type_size <= data.len() {
                        let ptr = val_or_ptr as usize;
                        for i in 0..count {
                            let off = if ftype == 3 {
                                read_u16(data, ptr + i * 2).unwrap_or(0) as u32
                            } else {
                                read_u32(data, ptr + i * 4).unwrap_or(0)
                            };
                            strip_offsets.push(off);
                        }
                    }
                } else if tag == 279 {
                    // StripByteCounts
                    if count == 1 {
                        strip_byte_counts.push(val_or_ptr);
                    } else if (val_or_ptr as usize) + count * type_size <= data.len() {
                        let ptr = val_or_ptr as usize;
                        for i in 0..count {
                            let cnt = if ftype == 3 {
                                read_u16(data, ptr + i * 2).unwrap_or(0) as u32
                            } else {
                                read_u32(data, ptr + i * 4).unwrap_or(0)
                            };
                            strip_byte_counts.push(cnt);
                        }
                    }
                }
            }

            for (off, cnt) in strip_offsets.iter().zip(strip_byte_counts.iter()) {
                let strip_end = (*off as usize).saturating_add(*cnt as usize);
                max_extent = max_extent.max(strip_end);
            }

            let next_ifd = read_u32(data, ifd_offset + 2 + num_entries * 12).unwrap_or(0) as usize;
            if next_ifd == 0 || next_ifd <= ifd_offset {
                break;
            }
            ifd_offset = next_ifd;
        }

        if max_extent >= 8 && max_extent <= data.len() {
            Some(max_extent)
        } else {
            None
        }
    }

    /// Structure-aware PCX parser verifying RLE scanline decoding to calculate exact byte length
    fn parse_pcx_size_slice(data: &[u8]) -> Option<usize> {
        if data.len() < 128 || data[0] != 0x0A {
            return None;
        }

        let version = data[1];
        let xmin = u16::from_le_bytes([data[4], data[5]]) as usize;
        let ymin = u16::from_le_bytes([data[6], data[7]]) as usize;
        let xmax = u16::from_le_bytes([data[8], data[9]]) as usize;
        let ymax = u16::from_le_bytes([data[10], data[11]]) as usize;
        let planes = data[65] as usize;
        let bytes_per_line = u16::from_le_bytes([data[66], data[67]]) as usize;

        if xmax < xmin || ymax < ymin || planes == 0 || bytes_per_line == 0 {
            return None;
        }

        let height = (ymax - ymin) + 1;
        let scanline_bytes = bytes_per_line * planes;
        let total_uncompressed_bytes = scanline_bytes * height;

        let mut uncompressed_count = 0usize;
        let mut cur = 128usize;

        while cur < data.len() && uncompressed_count < total_uncompressed_bytes {
            let b = data[cur];
            cur += 1;
            if (b & 0xC0) == 0xC0 {
                let count = (b & 0x3F) as usize;
                if cur >= data.len() {
                    break;
                }
                cur += 1; // pixel value
                uncompressed_count += count;
            } else {
                uncompressed_count += 1;
            }
        }

        if uncompressed_count >= total_uncompressed_bytes {
            // Check for 256-color palette (0x0C + 768 bytes) in Version 5
            if version == 5 && cur < data.len() && data[cur] == 0x0C && cur + 769 <= data.len() {
                cur += 769;
            }
            Some(cur)
        } else {
            None
        }
    }

    /// Inspect extracted or mapped ZIP archive to classify OOXML Office documents (.docx, .xlsx, .pptx)
    fn detect_ooxml_type_from_path(path: &Path) -> Option<SupportedFileType> {
        let mut f = File::open(path).ok()?;
        let mut header = [0u8; 16 * 1024];
        let n = f.read(&mut header).ok()?;
        let slice = &header[..n];

        // First verify standard ZIP local header
        if slice.len() < 30 || &slice[..4] != &[0x50, 0x4B, 0x03, 0x04] {
            return None;
        }

        // Must contain [Content_Types].xml or root relationship parts
        let has_content_types = slice.windows(19).any(|w| w == b"[Content_Types].xml");
        let has_rels = slice.windows(6).any(|w| w == b"_rels/");

        if !has_content_types && !has_rels {
            return None;
        }

        if slice.windows(5).any(|w| w == b"word/") {
            Some(SupportedFileType::Docx)
        } else if slice.windows(3).any(|w| w == b"xl/") {
            Some(SupportedFileType::Xlsx)
        } else if slice.windows(4).any(|w| w == b"ppt/") {
            Some(SupportedFileType::Pptx)
        } else {
            None
        }
    }

    /// Inspect extracted ASF container to classify whether it is WMA (audio) or WMV (video)
    fn detect_asf_type_from_path(path: &Path) -> Option<SupportedFileType> {
        let mut f = File::open(path).ok()?;
        let mut header = [0u8; 16 * 1024];
        let n = f.read(&mut header).ok()?;
        let slice = &header[..n];

        // ASF Video Media GUID: BC 19 EF C0 5B 4D 11 CF A8 FD 00 80 5F 5C 44 2B
        const ASF_VIDEO_GUID: &[u8; 16] = &[
            0xC0, 0xEF, 0x19, 0xBC, 0x4D, 0x5B, 0xCF, 0x11,
            0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B,
        ];
        if slice.windows(16).any(|w| w == ASF_VIDEO_GUID) {
            return Some(SupportedFileType::Wmv);
        }

        // ASF Audio Media GUID: F8 69 9E 40 5B 4D 11 CF A8 FD 00 80 5F 5C 44 2B
        const ASF_AUDIO_GUID: &[u8; 16] = &[
            0x40, 0x9E, 0x69, 0xF8, 0x4D, 0x5B, 0xCF, 0x11,
            0xA8, 0xFD, 0x00, 0x80, 0x5F, 0x5C, 0x44, 0x2B,
        ];
        if slice.windows(16).any(|w| w == ASF_AUDIO_GUID) {
            return Some(SupportedFileType::Wma);
        }

        None
    }

    /// Parse total file size from ASF / WMA / WMV header slice
    fn parse_asf_size_slice(slice: &[u8]) -> Option<usize> {
        // ASF File Properties Object GUID: A1 DC AB 8C 47 A9 CF 11 8E E4 00 C0 0C 20 53 65
        const FILE_PROP_GUID: &[u8; 16] = &[
            0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11,
            0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65,
        ];
        if let Some(pos) = slice.windows(16).position(|w| w == FILE_PROP_GUID) {
            if pos + 48 <= slice.len() {
                let file_size = u64::from_le_bytes(slice[pos + 40..pos + 48].try_into().unwrap_or([0; 8])) as usize;
                if file_size >= 100 {
                    return Some(file_size);
                }
            }
        }
        None
    }

    /// Parse Sun/NeXT AU file size from 24-byte header slice
    fn parse_au_size_slice(slice: &[u8]) -> Option<usize> {
        if slice.len() < 24 || &slice[..4] != b".snd" {
            return None;
        }
        let data_offset = u32::from_be_bytes(slice[4..8].try_into().unwrap_or([0; 4])) as usize;
        let data_size = u32::from_be_bytes(slice[8..12].try_into().unwrap_or([0; 4])) as usize;
        if data_size != 0xFFFF_FFFF && data_size > 0 {
            let total = data_offset.saturating_add(data_size);
            if total >= 24 {
                return Some(total);
            }
        }
        None
    }

    /// Search for bzip2 48-bit End-of-Stream marker (0x177245385090) across all 8 bit shifts
    fn find_bz2_eos_in_slice(data: &[u8]) -> Option<usize> {
        if data.len() < 10 {
            return None;
        }
        // Bit-shifted 48-bit EOS markers for MSB-first packing (7 byte prefix)
        const SHIFTED_PATTERNS: [&[u8; 6]; 8] = [
            &[0x17, 0x72, 0x45, 0x38, 0x50, 0x90], // shift 0
            &[0x2E, 0xE4, 0x8A, 0x70, 0xA1, 0x20], // shift 1
            &[0x5D, 0xC9, 0x14, 0xE1, 0x42, 0x40], // shift 2
            &[0xBB, 0x92, 0x29, 0xC2, 0x84, 0x80], // shift 3
            &[0x77, 0x24, 0x53, 0x85, 0x09, 0x00], // shift 4
            &[0xEE, 0x48, 0xA7, 0x0A, 0x12, 0x00], // shift 5
            &[0xDC, 0x91, 0x4E, 0x14, 0x24, 0x00], // shift 6
            &[0xB9, 0x22, 0x9C, 0x28, 0x48, 0x00], // shift 7
        ];

        // Also check bit-by-bit via u64 sliding window for exact bit boundary match
        let target_magic: u64 = 0x1772_4538_5090;
        let mut accum: u64 = 0;
        for i in 0..6 {
            accum = (accum << 8) | data[i] as u64;
        }

        if (accum & 0xFFFF_FFFF_FFFF) == target_magic {
            return Some(10); // 6 bytes EOS + 4 bytes CRC
        }

        for (byte_idx, &b) in data[6..].iter().enumerate() {
            for bit in (0..8).rev() {
                let bit_val = ((b >> bit) & 1) as u64;
                accum = ((accum << 1) | bit_val) & 0xFFFF_FFFF_FFFF;
                if accum == target_magic {
                    // Match found! Total file extent ends after the 32-bit CRC and byte padding
                    let current_byte_pos = 6 + byte_idx + 1;
                    return Some(current_byte_pos + 4);
                }
            }
        }

        // Fast fallback check against static byte patterns
        for pattern in &SHIFTED_PATTERNS {
            if let Some(pos) = data.windows(6).position(|w| w == *pattern) {
                return Some(pos + 10);
            }
        }

        None
    }
    /// Skips past APP0..APP15 segments (where thumbnails live) and stops at true entropy EOI (0xFF 0xD9).
    fn determine_jpeg_size_direct(handle: HANDLE, offset: u64, end_boundary: u64, scratch: &mut AlignedBuffer) -> Option<usize> {
        let chunk_size = 64 * 1024;
        let to_read = ((end_boundary - offset) as usize).min(chunk_size);
        let initial_data = Self::read_direct_aligned_slice(handle, offset, to_read, scratch).ok()?;
        if initial_data.len() < 4 || initial_data[0] != 0xFF || initial_data[1] != 0xD8 {
            return None;
        }

        let mut idx = 2;
        let mut in_entropy = false;
        let mut has_sof = false;

        while idx + 4 <= initial_data.len() {
            if initial_data[idx] != 0xFF {
                if let Some(pos) = initial_data[idx..].iter().position(|&b| b == 0xFF) {
                    idx += pos;
                    continue;
                } else {
                    break;
                }
            }

            let marker = initial_data[idx + 1];

            if (0xC0..=0xC3).contains(&marker) || (0xC9..=0xCB).contains(&marker) {
                has_sof = true;
            }

            if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
                idx += 2;
                continue;
            }

            let seg_len = u16::from_be_bytes([initial_data[idx + 2], initial_data[idx + 3]]) as usize;
            if seg_len < 2 {
                idx += 2;
                continue;
            }

            if marker == 0xDA {
                idx += 2 + seg_len;
                in_entropy = true;
                break;
            }

            idx += 2 + seg_len;
        }

        if in_entropy {
            for i in idx..initial_data.len().saturating_sub(1) {
                if initial_data[i] == 0xFF && initial_data[i + 1] == 0xD9 {
                    let total = i + 2;
                    if has_sof && total >= 4096 {
                        return Some(total);
                    }
                }
            }
        }

        let mut cur_pos = offset + idx as u64;
        while cur_pos < end_boundary {
            let to_read = ((end_boundary - cur_pos) as usize).min(chunk_size);
            let slice_data = Self::read_direct_aligned_slice(handle, cur_pos, to_read, scratch).ok()?;
            if slice_data.len() < 2 {
                break;
            }
            for i in 0..slice_data.len().saturating_sub(1) {
                if slice_data[i] == 0xFF && slice_data[i + 1] == 0xD9 {
                    let total = (cur_pos + i as u64 + 2 - offset) as usize;
                    if has_sof && total >= 4096 {
                        return Some(total);
                    }
                }
            }
            cur_pos += slice_data.len().saturating_sub(1) as u64;
        }

        None
    }

    fn extract_file_direct(
        &self,
        handle: HANDLE,
        offset: u64,
        size: usize,
        out_path: &Path,
        scratch: &mut AlignedBuffer,
    ) -> Result<usize> {
        let mut out = match File::create(out_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let stem = out_path.file_stem().unwrap_or_default().to_string_lossy();
                let ext = out_path.extension().unwrap_or_default().to_string_lossy();
                let alt_path = out_path.parent().unwrap_or(Path::new(".")).join(format!("{}_alt.{}", stem, ext));
                File::create(alt_path)?
            }
            Err(e) => return Err(e.into()),
        };
        if self.sparse_enabled {
            crate::core::imager::cloner::mark_file_sparse(&out);
        }
        let mut remaining = size;
        let mut cur_offset = offset;

        while remaining > 0 {
            let to_read = remaining.min(64 * 1024);
            let chunk = match Self::read_direct_aligned_slice(handle, cur_offset, to_read, scratch) {
                Ok(c) => c,
                Err(err) => {
                    tracing::warn!("Extraction read error {} at 0x{:X}", err, cur_offset);
                    break;
                }
            };

            if chunk.is_empty() {
                break;
            }

            out.write_all(&chunk)?;
            cur_offset += chunk.len() as u64;
            remaining -= chunk.len();
        }

        out.flush()?;
        Ok(size - remaining)
    }

    /// Carve from a raw image file on disk (supports .raw and .img files)
    pub fn carve_from_file<P: AsRef<Path>>(
        &self,
        path: P,
        tx: Option<Sender<CarverEvent>>,
        pause: Option<Arc<AtomicBool>>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<CarvedFile>> {
        let path_ref = path.as_ref();
        if path_ref.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("'{}' is a directory! Please specify the exact .raw or .img file.", path_ref.display()),
            ).into());
        }

        #[cfg(windows)]
        let mut file = {
            let mut opts = std::fs::OpenOptions::new();
            opts.read(true);
            opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
            opts.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_SEQUENTIAL_SCAN);
            opts.open(path_ref).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("Cannot open image file '{}': {}", path_ref.display(), e),
                )
            })?
        };

        #[cfg(not(windows))]
        let mut file = File::open(path_ref).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("Cannot open image file '{}': {}", path_ref.display(), e),
            )
        })?;

        let total_size = file.metadata().map(|m| m.len()).unwrap_or(0);
        fs::create_dir_all(&self.output_dir).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("Cannot create output directory '{}': {}", self.output_dir.display(), e),
            )
        })?;

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Started { total_bytes: total_size });
        }

        let mut carved_files = Vec::new();
        let chunk_size = 1024 * 1024;
        let mut buffer = vec![0u8; chunk_size];
        let mut current_offset = 0u64;

        let mut trim_guard = TrimGuard::new(self.is_ssd, 32);
        let mut consecutive_trim_bails = 0;

        let mut last_progress_time = Instant::now();
        let mut last_progress_bytes = 0u64;

        while current_offset < total_size && carved_files.len() < self.max_files {
            if let Some(ref c) = cancel {
                if c.load(Ordering::Relaxed) {
                    tracing::info!("Carver file session cancelled by user.");
                    break;
                }
            }

            while let Some(ref p) = pause {
                if p.load(Ordering::Relaxed) {
                    if let Some(ref c) = cancel {
                        if c.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                } else {
                    break;
                }
            }

            file.seek(SeekFrom::Start(current_offset))?;
            let bytes_read = match file.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("File read error {} at 0x{:X}", e, current_offset);
                    current_offset += 4096;
                    continue;
                }
            };
            if bytes_read == 0 {
                break;
            }

            let window = &buffer[..bytes_read];

            if trim_guard.inspect_block(window) {
                consecutive_trim_bails += 1;
                current_offset += 16 * 1024 * 1024;
                continue;
            }

            let mut i = 0;
            while i < bytes_read.saturating_sub(16) && carved_files.len() < self.max_files {
                let file_offset = current_offset + i as u64;
                let current_lba = file_offset / 512;
                if let Some(ref tracker) = self.claimed_tracker {
                    if tracker.is_claimed(current_lba) {
                        let rem = 512 - (i % 512);
                        i += rem;
                        continue;
                    }
                }

                for &ft in &self.target_types {
                    if ft.matches_at(window, i) {
                        let file_offset = current_offset + i as u64;
                        if let Some(size) = self.determine_file_size_file(&mut file, file_offset, ft) {
                            let filename = format!("carved_{:010}_{}.{}", file_offset, carved_files.len() + 1, ft.extension());
                            let out_path = self.output_dir.join(filename);

                            if let Ok(extracted_size) = self.extract_file_file(&mut file, file_offset, size, &out_path) {
                                // OOXML Auto-Detection: if carved as generic Zip, inspect inner payload to reclassify to docx/xlsx/pptx
                                let (final_type, final_path) = if ft == SupportedFileType::Zip {
                                    if let Some(ooxml) = Self::detect_ooxml_type_from_path(&out_path) {
                                        let new_filename = format!("carved_{:010}_{}.{}", file_offset, carved_files.len() + 1, ooxml.extension());
                                        let new_path = self.output_dir.join(new_filename);
                                        if fs::rename(&out_path, &new_path).is_ok() {
                                            (ooxml, new_path)
                                        } else {
                                            (ft, out_path)
                                        }
                                    } else {
                                        (ft, out_path)
                                    }
                                } else if ft == SupportedFileType::Wma || ft == SupportedFileType::Wmv {
                                    if let Some(asf_type) = Self::detect_asf_type_from_path(&out_path) {
                                        let new_filename = format!("carved_{:010}_{}.{}", file_offset, carved_files.len() + 1, asf_type.extension());
                                        let new_path = self.output_dir.join(new_filename);
                                        if fs::rename(&out_path, &new_path).is_ok() {
                                            (asf_type, new_path)
                                        } else {
                                            (ft, out_path)
                                        }
                                    } else {
                                        (ft, out_path)
                                    }
                                } else {
                                    (ft, out_path)
                                };

                                let cf = CarvedFile {
                                    file_type: final_type,
                                    offset: file_offset,
                                    size_bytes: extracted_size,
                                    output_path: final_path,
                                };
                                if let Some(ref sender) = tx {
                                    let _ = sender.send(CarverEvent::FileFound(cf.clone()));
                                }
                                carved_files.push(cf);
                                i += size.max(1);
                                break;
                            }
                        }
                    }
                }
                i += 512;
            }

            current_offset += (bytes_read.saturating_sub(4096)).max(4096) as u64;

            if last_progress_time.elapsed().as_millis() >= 200 {
                let elapsed_sec = last_progress_time.elapsed().as_secs_f64();
                let bytes_delta = current_offset.saturating_sub(last_progress_bytes);
                let speed_mb_s = if elapsed_sec > 0.0 { (bytes_delta as f64 / 1e6) / elapsed_sec } else { 0.0 };

                if let Some(ref sender) = tx {
                    let _ = sender.send(CarverEvent::Progress {
                        scanned_bytes: current_offset,
                        total_bytes: total_size,
                        speed_mb_s,
                        files_found: carved_files.len(),
                    });
                }

                last_progress_time = Instant::now();
                last_progress_bytes = current_offset;
            }
        }

        if consecutive_trim_bails > 0 {
            tracing::info!("TRIM Guard triggered: skipped {} blocks discarded by SSD deallocation.", consecutive_trim_bails);
        }

        if let Some(ref sender) = tx {
            let _ = sender.send(CarverEvent::Completed { total_found: carved_files.len() });
        }

        Ok(carved_files)
    }

    fn determine_file_size_file(&self, file: &mut File, offset: u64, file_type: SupportedFileType) -> Option<usize> {
        let max_len = file_type.max_size_bytes();
        let mut search_buf = vec![0u8; 64 * 1024];

        match file_type {
            SupportedFileType::Tiff => {
                let to_read = (50 * 1024 * 1024).min(max_len);
                let mut buf = vec![0u8; to_read];
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    if let Ok(n) = file.read(&mut buf) {
                        return Self::parse_tiff_size_slice(&buf[..n]);
                    }
                }
                None
            }
            SupportedFileType::Pcx => {
                let to_read = (50 * 1024 * 1024).min(max_len);
                let mut buf = vec![0u8; to_read];
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    if let Ok(n) = file.read(&mut buf) {
                        return Self::parse_pcx_size_slice(&buf[..n]);
                    }
                }
                None
            }
            SupportedFileType::Jpeg => {
                let initial_size = (64 * 1024).min(max_len);
                if file.seek(SeekFrom::Start(offset)).is_err() {
                    return None;
                }
                let n = file.read(&mut search_buf[..initial_size]).ok()?;
                let initial_data = &search_buf[..n];
                if initial_data.len() < 4 || initial_data[0] != 0xFF || initial_data[1] != 0xD8 {
                    return None;
                }

                let mut idx = 2;
                let mut in_entropy = false;
                let mut has_sof = false;

                while idx + 4 <= initial_data.len() {
                    if initial_data[idx] != 0xFF {
                        if let Some(pos) = initial_data[idx..].iter().position(|&b| b == 0xFF) {
                            idx += pos;
                            continue;
                        } else {
                            break;
                        }
                    }

                    let marker = initial_data[idx + 1];

                    if (0xC0..=0xC3).contains(&marker) || (0xC9..=0xCB).contains(&marker) {
                        has_sof = true;
                    }

                    if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
                        idx += 2;
                        continue;
                    }

                    let seg_len = u16::from_be_bytes([initial_data[idx + 2], initial_data[idx + 3]]) as usize;
                    if seg_len < 2 {
                        idx += 2;
                        continue;
                    }

                    if marker == 0xDA {
                        idx += 2 + seg_len;
                        in_entropy = true;
                        break;
                    }

                    idx += 2 + seg_len;
                }

                if in_entropy {
                    for i in idx..initial_data.len().saturating_sub(1) {
                        if initial_data[i] == 0xFF && initial_data[i + 1] == 0xD9 {
                            let total = i + 2;
                            if has_sof && total >= 4096 {
                                return Some(total);
                            }
                        }
                    }
                }

                let mut cur_pos = offset + idx as u64;
                while cur_pos < offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(cur_pos)).is_err() {
                        break;
                    }
                    let read_bytes = file.read(&mut search_buf).ok()?;
                    if read_bytes < 2 {
                        break;
                    }
                    let slice = &search_buf[..read_bytes];
                    for i in 0..slice.len().saturating_sub(1) {
                        if slice[i] == 0xFF && slice[i + 1] == 0xD9 {
                            let total = (cur_pos + i as u64 + 2 - offset) as usize;
                            if has_sof && total >= 4096 {
                                return Some(total);
                            }
                        }
                    }
                    cur_pos += slice.len().saturating_sub(1) as u64;
                }

                None
            }
            SupportedFileType::Png => {
                let iend_pattern = &[0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82];
                let mut pos = offset + 8;
                while pos < offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(pos)).is_err() {
                        break;
                    }
                    let n = file.read(&mut search_buf).ok()?;
                    if n < 8 {
                        break;
                    }
                    for idx in 0..n.saturating_sub(7) {
                        if &search_buf[idx..idx + 8] == iend_pattern {
                            return Some((pos + idx as u64 + 8 - offset) as usize);
                        }
                    }
                    pos += (n - 7) as u64;
                }
                None
            }
            SupportedFileType::Gif => {
                let to_read = (10 * 1024 * 1024).min(max_len);
                let mut buf = vec![0u8; to_read];
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    if let Ok(n) = file.read(&mut buf) {
                        return Self::parse_gif_size_slice(&buf[..n]);
                    }
                }
                None
            }
            SupportedFileType::Bmp => {
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut search_buf[..54]).is_ok() {
                    let size = u32::from_le_bytes(search_buf[2..6].try_into().unwrap_or([0; 4])) as usize;
                    let reserved1 = u16::from_le_bytes([search_buf[6], search_buf[7]]);
                    let reserved2 = u16::from_le_bytes([search_buf[8], search_buf[9]]);
                    let off_bits = u32::from_le_bytes(search_buf[10..14].try_into().unwrap_or([0; 4])) as usize;
                    let bi_size = u32::from_le_bytes(search_buf[14..18].try_into().unwrap_or([0; 4]));
                    let bi_planes = u16::from_le_bytes([search_buf[26], search_buf[27]]);
                    let bi_bit_count = u16::from_le_bytes([search_buf[28], search_buf[29]]);

                    if reserved1 == 0
                        && reserved2 == 0
                        && (off_bits == 54 || off_bits == 62 || off_bits == 118 || off_bits == 124 || off_bits == 138)
                        && (bi_size == 40 || bi_size == 108 || bi_size == 124 || bi_size == 12 || bi_size == 52 || bi_size == 56 || bi_size == 64)
                        && bi_planes == 1
                        && matches!(bi_bit_count, 1 | 4 | 8 | 16 | 24 | 32)
                        && size >= off_bits
                        && size <= max_len
                    {
                        return Some(size);
                    }
                }
                None
            }
            SupportedFileType::Gz => {
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut search_buf[..10]).is_ok() {
                    if search_buf[0] == 0x1F && search_buf[1] == 0x8B && search_buf[2] == 0x08 {
                        let flags = search_buf[3];
                        if flags & 0xE0 != 0 {
                            return None;
                        }
                        let mut pos = offset + 10;
                        let scan_limit = offset + max_len as u64;
                        let mut last_non_zero = pos;
                        while pos < scan_limit {
                            if file.seek(SeekFrom::Start(pos)).is_err() {
                                break;
                            }
                            let to_read = ((scan_limit - pos) as usize).min(search_buf.len());
                            let n = file.read(&mut search_buf[..to_read]).ok()?;
                            if n == 0 {
                                break;
                            }
                            let mut zero_run = 0;
                            for (idx, &b) in search_buf[..n].iter().enumerate() {
                                if b == 0 {
                                zero_run += 1;
                                if zero_run >= 4096 {
                                    let end_file = (pos + idx as u64 + 1 - 4096).max(last_non_zero);
                                    return Some((end_file - offset) as usize);
                                }
                            } else {
                                zero_run = 0;
                                last_non_zero = pos + idx as u64 + 1;
                            }
                        }
                        pos += n as u64;
                    }
                    if last_non_zero > offset + 10 {
                        return Some(((last_non_zero - offset) as usize).min(max_len));
                    }
                }
            }
            None
        }
        SupportedFileType::Mp4 | SupportedFileType::Mov => {
            let mut cur = offset;
            let mut found_moov = false;
            let mut found_mdat = false;
            let mut last_valid_end = offset;
            let mut hdr_buf = [0u8; 8];

            while cur < offset + max_len as u64 {
                if file.seek(SeekFrom::Start(cur)).is_err() || file.read_exact(&mut hdr_buf).is_err() {
                    break;
                }
                let box_size = u32::from_be_bytes(hdr_buf[0..4].try_into().unwrap_or([0; 4])) as u64;
                let box_type = &hdr_buf[4..8];

                const VALID_MP4_BOXES: &[&[u8; 4]] = &[
                    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"uuid", b"meta", b"pdin", b"styp", b"sidx", b"moof", b"mvex",
                ];
                let valid_type = VALID_MP4_BOXES.iter().any(|b| &box_type == b);
                if !valid_type {
                    break;
                }

                if box_type == b"moov" {
                    found_moov = true;
                } else if box_type == b"mdat" {
                    found_mdat = true;
                }

                if box_size == 0 {
                    if found_moov && found_mdat {
                        last_valid_end = cur;
                    }
                    break;
                }

                let actual_size = if box_size == 1 {
                    let mut ext_buf = [0u8; 8];
                    if file.seek(SeekFrom::Start(cur + 8)).is_err() || file.read_exact(&mut ext_buf).is_err() {
                        break;
                    }
                    u64::from_be_bytes(ext_buf)
                } else {
                    box_size
                };

                if box_type == b"moov" && actual_size >= 16 {
                    let check_len = (actual_size as usize).min(256);
                    let mut moov_preview = vec![0u8; check_len - 8];
                    if file.seek(SeekFrom::Start(cur + 8)).is_ok() && file.read_exact(&mut moov_preview).is_ok() {
                        if !moov_preview.windows(4).any(|w| w == b"trak" || w == b"mvhd") {
                            found_moov = false;
                        }
                    }
                }

                if actual_size < 8 || actual_size > max_len as u64 {
                    break;
                }

                cur += actual_size;
                last_valid_end = cur;
            }

            if found_moov && found_mdat && last_valid_end > offset {
                Some(((last_valid_end - offset) as usize).min(max_len))
            } else {
                None
            }
        }
            SupportedFileType::Pdf => {
                let mut pos = offset + 5;
                let mut last_eof = None;
                while pos < offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(pos)).is_err() {
                        break;
                    }
                    let n = file.read(&mut search_buf).ok()?;
                    if n < 5 {
                        break;
                    }
                    for idx in 0..n.saturating_sub(4) {
                        if &search_buf[idx..idx + 5] == b"%%EOF" {
                            last_eof = Some((pos + idx as u64 + 5 - offset) as usize);
                        }
                    }
                    pos += (n - 4) as u64;
                }
                last_eof
            }
            SupportedFileType::Zip | SupportedFileType::Docx | SupportedFileType::Xlsx | SupportedFileType::Pptx => {
                let eocd_pattern = &[0x50, 0x4B, 0x05, 0x06];
                let mut pos = offset + 4;
                while pos < offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(pos)).is_err() {
                        break;
                    }
                    let n = file.read(&mut search_buf).ok()?;
                    if n < 4 {
                        break;
                    }
                    for idx in 0..n.saturating_sub(3) {
                        if &search_buf[idx..idx + 4] == eocd_pattern {
                            let mut comment_len = 0usize;
                            if idx + 22 <= n {
                                comment_len = u16::from_le_bytes([search_buf[idx + 20], search_buf[idx + 21]]) as usize;
                            }
                            return Some((pos + idx as u64 + 22 + comment_len as u64 - offset) as usize);
                        }
                    }
                    pos += (n - 3) as u64;
                }
                None
            }
            SupportedFileType::SevenZip => {
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut search_buf[..32]).is_ok() {
                    let next_offset = u64::from_le_bytes(search_buf[12..20].try_into().unwrap_or([0; 8]));
                    let next_size = u64::from_le_bytes(search_buf[20..28].try_into().unwrap_or([0; 8]));
                    let total = 32 + next_offset + next_size;
                    if total <= max_len as u64 {
                        return Some(total as usize);
                    }
                }
                None
            }
            SupportedFileType::Avi | SupportedFileType::Wav | SupportedFileType::Webp => {
                let mut hdr = [0u8; 12];
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut hdr).is_ok() {
                    if &hdr[..4] == b"RIFF" {
                        let riff_len = u32::from_le_bytes(hdr[4..8].try_into().unwrap_or([0; 4])) as usize;
                        let total = riff_len + 8;
                        if total <= max_len && total >= 12 {
                            return Some(total);
                        }
                    }
                }
                None
            }
            SupportedFileType::Mkv => {
                Some(100 * 1024 * 1024)
            }
            SupportedFileType::Mp3 => {
                let mut hdr = [0u8; 10];
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut hdr).is_ok() {
                    if &hdr[..3] == b"ID3" {
                        let s0 = (hdr[6] & 0x7F) as usize;
                        let s1 = (hdr[7] & 0x7F) as usize;
                        let s2 = (hdr[8] & 0x7F) as usize;
                        let s3 = (hdr[9] & 0x7F) as usize;
                        let id3_size = (s0 << 21) | (s1 << 14) | (s2 << 7) | s3;
                        let id3_len = 10 + id3_size;

                        let to_read = (id3_len + 20 * 1024 * 1024).min(max_len);
                        let mut audio_buf = vec![0u8; to_read];
                        if file.seek(SeekFrom::Start(offset)).is_ok() {
                            if let Ok(n) = file.read(&mut audio_buf) {
                                return Self::parse_mp3_size_slice(&audio_buf[..n], id3_len);
                            }
                        }
                    }
                }
                None
            }
            SupportedFileType::Flac => {
                Some(30 * 1024 * 1024)
            }
            SupportedFileType::Ogg => {
                Some(20 * 1024 * 1024)
            }
            SupportedFileType::Psd => {
                Some(50 * 1024 * 1024)
            }
            SupportedFileType::Rtf => {
                Some(10 * 1024 * 1024)
            }
            SupportedFileType::Tar => {
                let mut cur = offset;
                let mut block = [0u8; 512];
                let mut consecutive_zero_blocks = 0;

                while cur + 512 <= offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(cur)).is_err() || file.read_exact(&mut block).is_err() {
                        break;
                    }
                    if block.iter().all(|&b| b == 0) {
                        consecutive_zero_blocks += 1;
                        cur += 512;
                        if consecutive_zero_blocks >= 2 {
                            return Some(((cur - offset) as usize).min(max_len));
                        }
                        continue;
                    } else {
                        consecutive_zero_blocks = 0;
                    }

                    let size_field = &block[124..136];
                    let size_str = String::from_utf8_lossy(size_field);
                    let trimmed = size_str.trim_matches(|c: char| c == '\0' || c == ' ');
                    if let Ok(file_size) = u64::from_str_radix(trimmed, 8) {
                        let blocks = (file_size + 511) / 512;
                        cur += 512 + (blocks * 512);
                    } else {
                        cur += 512;
                    }
                }
                if cur > offset {
                    Some(((cur - offset) as usize).min(max_len))
                } else {
                    Some(10 * 1024 * 1024)
                }
            }
            SupportedFileType::Bz2 => {
                let mut pos = offset + 4;
                while pos < offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(pos)).is_err() {
                        break;
                    }
                    let n = file.read(&mut search_buf).ok()?;
                    if n < 10 {
                        break;
                    }
                    if let Some(idx) = Self::find_bz2_eos_in_slice(&search_buf[..n]) {
                        return Some((pos + idx as u64 - offset) as usize);
                    }
                    pos += (n.saturating_sub(16)).max(1) as u64;
                }
                None
            }
            SupportedFileType::Wim => {
                let mut header = [0u8; 208];
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut header).is_ok() {
                    if &header[..8] == b"MSWIM\0\0\0" {
                        let lut_size = u64::from_le_bytes([header[48], header[49], header[50], header[51], header[52], header[53], header[54], 0]);
                        let lut_offset = u64::from_le_bytes(header[56..64].try_into().unwrap_or([0; 8]));
                        let total = lut_offset + lut_size;
                        if total > 0 && total <= max_len as u64 {
                            return Some(total as usize);
                        }
                    }
                }
                Some(50 * 1024 * 1024)
            }
            SupportedFileType::Au => {
                let mut header = [0u8; 24];
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut header).is_ok() {
                    return Self::parse_au_size_slice(&header);
                }
                None
            }
            SupportedFileType::Wma | SupportedFileType::Wmv => {
                let mut header = [0u8; 16 * 1024];
                if file.seek(SeekFrom::Start(offset)).is_ok() {
                    if let Ok(n) = file.read(&mut header) {
                        return Self::parse_asf_size_slice(&header[..n]);
                    }
                }
                None
            }
            SupportedFileType::Flv => {
                let mut cur = offset + 9;
                let mut last_tag_end = offset + 9;
                let mut tag_hdr = [0u8; 15];
                while cur + 15 <= offset + max_len as u64 {
                    if file.seek(SeekFrom::Start(cur)).is_err() || file.read_exact(&mut tag_hdr).is_err() {
                        break;
                    }
                    let tag_type = tag_hdr[4];
                    let data_size = ((tag_hdr[5] as usize) << 16) | ((tag_hdr[6] as usize) << 8) | (tag_hdr[7] as usize);

                    if tag_type != 8 && tag_type != 9 && tag_type != 18 {
                        break;
                    }
                    if data_size > 10 * 1024 * 1024 {
                        break;
                    }

                    let tag_total = 4 + 11 + data_size;
                    cur += tag_total as u64;
                    last_tag_end = cur;
                }
                if last_tag_end > offset + 13 {
                    Some(((last_tag_end + 4 - offset) as usize).min(max_len))
                } else {
                    None
                }
            }
            SupportedFileType::Mpg => {
                let mut pos = offset + 4;
                let scan_limit = offset + max_len as u64;
                while pos < scan_limit {
                    if file.seek(SeekFrom::Start(pos)).is_err() {
                        break;
                    }
                    let to_read = ((scan_limit - pos) as usize).min(search_buf.len());
                    let n = file.read(&mut search_buf[..to_read]).ok()?;
                    if n < 4 {
                        break;
                    }
                    for idx in 0..n.saturating_sub(3) {
                        if search_buf[idx] == 0x00 && search_buf[idx + 1] == 0x00 && search_buf[idx + 2] == 0x01 && search_buf[idx + 3] == 0xB9 {
                            return Some((pos + idx as u64 + 4 - offset) as usize);
                        }
                    }
                    pos += (n - 3) as u64;
                }
                Some(50 * 1024 * 1024)
            }
            SupportedFileType::Iso => {
                Some(500 * 1024 * 1024)
            }
            SupportedFileType::Txt => {
                let mut pos = offset;
                let scan_limit = offset + max_len as u64;
                let mut total_text_len = 0usize;

                while pos < scan_limit {
                    if file.seek(SeekFrom::Start(pos)).is_err() {
                        break;
                    }
                    let to_read = ((scan_limit - pos) as usize).min(search_buf.len());
                    let n = file.read(&mut search_buf[..to_read]).ok()?;
                    if n == 0 {
                        break;
                    }

                    let slice = &search_buf[..n];
                    let mut valid_in_chunk = 0;
                    for &b in slice {
                        if b == 0 || (b < 0x08 && b != 0x00) || (b > 0x0E && b < 0x1F) {
                            break;
                        }
                        if (0x20..=0x7E).contains(&b) || b == b'\t' || b == b'\r' || b == b'\n' || b >= 0xC0 {
                            valid_in_chunk += 1;
                        } else {
                            break;
                        }
                    }

                    total_text_len += valid_in_chunk;
                    if valid_in_chunk < n {
                        break;
                    }
                    pos += n as u64;
                }

                let min_gate = self.plaintext_gate.min_bytes().unwrap_or(512);
                if total_text_len >= min_gate {
                    Some(total_text_len)
                } else {
                    None
                }
            }
            SupportedFileType::Sqlite => {
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut search_buf[..32]).is_ok() {
                    let mut page_size = u16::from_be_bytes([search_buf[16], search_buf[17]]) as u64;
                    if page_size == 1 {
                        page_size = 65536;
                    }
                    let page_count = u32::from_be_bytes(search_buf[28..32].try_into().unwrap_or([0; 4])) as u64;
                    if page_size > 0 && page_count > 0 {
                        let total = page_size * page_count;
                        if total <= max_len as u64 {
                            return Some(total as usize);
                        }
                    }
                }
                None
            }
            SupportedFileType::Exe => {
                if file.seek(SeekFrom::Start(offset)).is_ok() && file.read_exact(&mut search_buf[..64]).is_ok() {
                    let pe_offset = u32::from_le_bytes(search_buf[0x3C..0x40].try_into().unwrap_or([0; 4])) as u64;
                    if pe_offset < 1024 && file.seek(SeekFrom::Start(offset + pe_offset)).is_ok() {
                        let mut pe_buf = [0u8; 256];
                        if file.read_exact(&mut pe_buf).is_ok() && &pe_buf[..4] == b"PE\0\0" {
                            let num_sections = u16::from_le_bytes([pe_buf[6], pe_buf[7]]) as usize;
                            let opt_hdr_size = u16::from_le_bytes([pe_buf[20], pe_buf[21]]) as u64;
                            let sec_table_offset = pe_offset + 24 + opt_hdr_size;
                            let mut sec_table = vec![0u8; num_sections * 40];
                            if file.seek(SeekFrom::Start(offset + sec_table_offset)).is_ok() && file.read_exact(&mut sec_table).is_ok() {
                                let mut max_end = sec_table_offset + (num_sections * 40) as u64;
                                for s in 0..num_sections {
                                    let b = s * 40;
                                    let raw_size = u32::from_le_bytes(sec_table[b + 16..b + 20].try_into().unwrap_or([0; 4])) as u64;
                                    let raw_ptr = u32::from_le_bytes(sec_table[b + 20..b + 24].try_into().unwrap_or([0; 4])) as u64;
                                    max_end = max_end.max(raw_ptr + raw_size);
                                }
                                if max_end > 0 && max_end <= max_len as u64 {
                                    return Some(max_end as usize);
                                }
                            }
                        }
                    }
                }
                None
            }
            _ => {
                // Fallback safe default
                Some(10 * 1024 * 1024)
            }
        }
    }

    fn extract_file_file(&self, file: &mut File, offset: u64, size: usize, out_path: &Path) -> Result<usize> {
        file.seek(SeekFrom::Start(offset))?;
        let mut out = match File::create(out_path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                let stem = out_path.file_stem().unwrap_or_default().to_string_lossy();
                let ext = out_path.extension().unwrap_or_default().to_string_lossy();
                let alt_path = out_path.parent().unwrap_or(Path::new(".")).join(format!("{}_alt.{}", stem, ext));
                File::create(alt_path)?
            }
            Err(e) => return Err(e.into()),
        };
        if self.sparse_enabled {
            crate::core::imager::cloner::mark_file_sparse(&out);
        }

        let mut remaining = size;
        let mut buf = vec![0u8; 64 * 1024];

        while remaining > 0 {
            let to_read = remaining.min(buf.len());
            let n = file.read(&mut buf[..to_read])?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            remaining -= n;
        }

        out.flush()?;
        Ok(size - remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_carver_detects_jpeg_with_exif_markers() {
        let temp_dir = std::env::temp_dir().join(format!("carver_test_{}", std::process::id()));
        let carver = SignatureCarver::new(temp_dir.clone(), false);

        // Construct synthetic stream: SOI + APP1 (with fake embedded thumbnail FF D9) + SOF0 + SOS + real EOI
        let mut dummy = Vec::new();
        dummy.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE1]); // SOI + APP1 marker
        dummy.extend_from_slice(&[0x00, 0x0A]);             // APP1 length: 10 bytes (including 2 bytes length)
        dummy.extend_from_slice(b"Exif\0\0");               // Header payload
        dummy.extend_from_slice(&[0xFF, 0xD9]);             // Fake embedded thumbnail EOI (MUST BE SKIPPED!)
        dummy.extend_from_slice(&[0xFF, 0xC0]);             // SOF0 (Start of Frame baseline)
        dummy.extend_from_slice(&[0x00, 0x0B]);             // SOF length: 11 bytes
        dummy.extend_from_slice(&[0x08, 0x00, 0x40, 0x00, 0x40, 0x03, 0x01, 0x11, 0x00]); // 64x64 baseline image
        dummy.extend_from_slice(&[0xFF, 0xDA]);             // Start of Scan (SOS)
        dummy.extend_from_slice(&[0x00, 0x08, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]); // SOS header
        dummy.extend_from_slice(&vec![0xAA; 4096]);         // Image entropy payload
        dummy.extend_from_slice(&[0xFF, 0xD9]);             // TRUE End of Image (EOI)!

        let dummy_path = temp_dir.join("test_jpeg.raw");
        let _ = fs::create_dir_all(&temp_dir);
        let mut f = File::create(&dummy_path).unwrap();
        f.write_all(&dummy).unwrap();
        drop(f);

        let carved = carver.carve_from_file(&dummy_path, None, None, None).unwrap();
        assert_eq!(carved.len(), 1);
        assert_eq!(carved[0].file_type, SupportedFileType::Jpeg);
        assert_eq!(carved[0].offset, 0);
        // The carved size must equal the full file length, skipping the embedded thumbnail!
        assert_eq!(carved[0].size_bytes, dummy.len());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_bmp_and_sqlite_exact_ruler_calculations() {
        let temp_dir = std::env::temp_dir().join(format!("carver_test_rules_{}", std::process::id()));
        let carver = SignatureCarver::new(temp_dir.clone(), false);

        // BMP header: "BM" + 4-byte filesize 0x00001000 (4096 bytes) + valid DIB header
        let mut bmp = vec![0u8; 4096];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&4096u32.to_le_bytes());
        bmp[6..8].copy_from_slice(&0u16.to_le_bytes());
        bmp[8..10].copy_from_slice(&0u16.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&32i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&32i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());

        let bmp_path = temp_dir.join("test.bmp.raw");
        let _ = fs::create_dir_all(&temp_dir);
        let mut f = File::create(&bmp_path).unwrap();
        f.write_all(&bmp).unwrap();
        drop(f);

        let carved = carver.carve_from_file(&bmp_path, None, None, None).unwrap();
        assert_eq!(carved.len(), 1);
        assert_eq!(carved[0].file_type, SupportedFileType::Bmp);
        assert_eq!(carved[0].size_bytes, 4096);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_mp4_rejects_missing_moov_and_accepts_valid_pair() {
        let temp_dir = std::env::temp_dir().join(format!("carver_mp4_test_{}", std::process::id()));
        let carver = SignatureCarver::new(temp_dir.clone(), false);
        let _ = fs::create_dir_all(&temp_dir);

        // 1. Incomplete MP4: ftyp + mdat ONLY (no moov atom) -> MUST BE REJECTED (unplayable)
        let mut unplayable = Vec::new();
        unplayable.extend_from_slice(&24u32.to_be_bytes()); // box size: 24
        unplayable.extend_from_slice(b"ftypisom");          // type + major brand
        unplayable.extend_from_slice(&[0, 0, 2, 0]);        // minor version
        unplayable.extend_from_slice(b"isommp41");          // compatible brands
        unplayable.extend_from_slice(&16u32.to_be_bytes()); // mdat box size: 16
        unplayable.extend_from_slice(b"mdat");
        unplayable.extend_from_slice(&[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]); // raw audio/video payload

        let bad_path = temp_dir.join("unplayable.mp4.raw");
        let mut f = File::create(&bad_path).unwrap();
        f.write_all(&unplayable).unwrap();
        drop(f);

        let carved_bad = carver.carve_from_file(&bad_path, None, None, None).unwrap();
        assert_eq!(carved_bad.len(), 0, "Unplayable MP4 without moov must be rejected");

        // 2. Complete MP4: ftyp + moov + mdat -> MUST BE CARVED
        let mut playable = unplayable.clone();
        playable.extend_from_slice(&16u32.to_be_bytes()); // moov box size: 16
        playable.extend_from_slice(b"moov");
        playable.extend_from_slice(b"mvhd\0\0\0\0"); // valid movie header metadata inside moov

        let good_path = temp_dir.join("playable.mp4.raw");
        let mut f = File::create(&good_path).unwrap();
        f.write_all(&playable).unwrap();
        drop(f);

        let carved_good = carver.carve_from_file(&good_path, None, None, None).unwrap();
        assert_eq!(carved_good.len(), 1, "Playable MP4 with both moov and mdat must be carved");
        assert_eq!(carved_good[0].file_type, SupportedFileType::Mp4);
        assert_eq!(carved_good[0].size_bytes, playable.len());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_plaintext_heuristic_carving_opt_in() {
        let temp_dir = std::env::temp_dir().join(format!("carver_txt_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("sample.raw");

        // Create 1024 bytes of clean readable text followed by nulls
        let mut data = vec![0u8; 4096];
        let text = b"The quick brown fox jumps over the lazy dog. Important recovery notes for AegisDisk.\n";
        for (i, b) in text.iter().cycle().take(1024).enumerate() {
            data[i] = *b;
        }

        let mut f = File::create(&test_file).unwrap();
        f.write_all(&data).unwrap();
        drop(f);

        // Without opt-in: Txt must not be carved
        let carver_no_txt = SignatureCarver::new(temp_dir.clone(), false);
        let res_no_txt = carver_no_txt.carve_from_file(&test_file, None, None, None).unwrap();
        assert_eq!(res_no_txt.len(), 0, "Plaintext should be ignored when opt-in is false");

        // With opt-in (1 KB Gate): Txt must be carved
        let carver_with_txt = SignatureCarver::new_with_options(temp_dir.clone(), false, PlaintextGate::Code1024);
        let res_with_txt = carver_with_txt.carve_from_file(&test_file, None, None, None).unwrap();
        assert_eq!(res_with_txt.len(), 1, "Plaintext should be carved when opt-in is true");
        assert_eq!(res_with_txt[0].file_type, SupportedFileType::Txt);
        assert_eq!(res_with_txt[0].size_bytes, 1024);

        // With 4 KB Gate: 1024B text must be ignored
        let carver_4k = SignatureCarver::new_with_options(temp_dir.clone(), false, PlaintextGate::Bulk4096);
        let res_4k = carver_4k.carve_from_file(&test_file, None, None, None).unwrap();
        assert_eq!(res_4k.len(), 0, "1KB text must be rejected under 4KB gate");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_gif_subblock_parser_ignores_internal_trailer_byte() {
        // GIF with an internal 0x3B embedded in raster data sub-block, terminated by 0x00 then 0x3B
        let gif = vec![
            b'G', b'I', b'F', b'8', b'9', b'a',
            10, 0, 10, 0, // 10x10
            0x80, // GCT present, 2 entries (6 bytes)
            0, 0,
            255, 0, 0, 0, 255, 0, // 6 bytes of GCT
            0x2C, // Image descriptor
            0, 0, 0, 0, 10, 0, 10, 0, 0, // descriptor fields
            2, // LZW min code size
            4, 0x01, 0x3B, 0x02, 0x03, // Sub-block with 0x3B inside payload!
            0, // Sub-block terminator
            0x3B, // True trailer
        ];

        let parsed_size = SignatureCarver::parse_gif_size_slice(&gif);
        assert_eq!(parsed_size, Some(gif.len()), "GIF sub-block parser must navigate past internal 0x3B payload byte to true trailer");
    }

    #[test]
    fn test_claimed_lba_skips_signature() {
        use crate::core::recovery::metadata::ClaimedLbaTracker;

        let temp_dir = std::env::temp_dir().join(format!("carver_claim_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let test_file = temp_dir.join("image.raw");

        // Construct valid BMP image
        let mut bmp = vec![0u8; 4096];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&4096u32.to_le_bytes());
        bmp[6..8].copy_from_slice(&0u16.to_le_bytes());
        bmp[8..10].copy_from_slice(&0u16.to_le_bytes());
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&32i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&32i32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());

        let mut f = File::create(&test_file).unwrap();
        f.write_all(&bmp).unwrap();
        drop(f);

        // Without claimed tracker: recovers 1 BMP
        let carver_plain = SignatureCarver::new(temp_dir.clone(), false);
        let res_plain = carver_plain.carve_from_file(&test_file, None, None, None).unwrap();
        assert_eq!(res_plain.len(), 1, "Must carve BMP when LBA is unclaimed");

        // With claimed tracker claiming LBA 0: skipped completely
        let tracker = Arc::new(ClaimedLbaTracker::new());
        tracker.claim_range(0, 8); // claim LBA 0..8
        let carver_claimed = SignatureCarver::new(temp_dir.clone(), false)
            .with_claimed_tracker(tracker);
        let res_claimed = carver_claimed.carve_from_file(&test_file, None, None, None).unwrap();
        assert_eq!(res_claimed.len(), 0, "Must skip LBA when claimed by metadata recovery");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
