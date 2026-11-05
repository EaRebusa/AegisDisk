use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use crate::common::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStatus {
    NonTried,    // '?'
    NonTrimmed,  // '*'
    NonScraped,  // '/'
    Bad,         // '-'
    Finished,    // '+'
}

impl BlockStatus {
    pub fn to_char(self) -> char {
        match self {
            Self::NonTried => '?',
            Self::NonTrimmed => '*',
            Self::NonScraped => '/',
            Self::Bad => '-',
            Self::Finished => '+',
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '?' => Some(Self::NonTried),
            '*' => Some(Self::NonTrimmed),
            '/' => Some(Self::NonScraped),
            '-' => Some(Self::Bad),
            '+' => Some(Self::Finished),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MapEntry {
    pub pos: u64,
    pub size: u64,
    pub status: BlockStatus,
}

#[derive(Debug, Clone)]
pub struct RescueMap {
    pub current_pos: u64,
    pub current_pass: u8,
    pub entries: Vec<MapEntry>,
    pub file_path: PathBuf,
}

impl RescueMap {
    pub fn new(total_bytes: u64, file_path: PathBuf) -> Self {
        let entries = vec![MapEntry {
            pos: 0,
            size: total_bytes,
            status: BlockStatus::NonTried,
        }];

        Self {
            current_pos: 0,
            current_pass: 1,
            entries,
            file_path,
        }
    }

    pub fn load_or_create(total_bytes: u64, file_path: PathBuf) -> Self {
        if file_path.exists() {
            if let Ok(map) = Self::load(&file_path) {
                return map;
            }
        }
        Self::new(total_bytes, file_path)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);

        let mut current_pos = 0u64;
        let mut current_pass = 1u8;
        let mut entries = Vec::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if let Some(pos_str) = trimmed.strip_prefix("current_pos = ") {
                current_pos = pos_str.trim().parse().unwrap_or(0);
                continue;
            }
            if let Some(pass_str) = trimmed.strip_prefix("current_pass = ") {
                current_pass = pass_str.trim().parse().unwrap_or(1);
                continue;
            }

            // Entry format: 0x000000 0x10000 +
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() == 3 {
                let pos = u64::from_str_radix(parts[0].trim_start_matches("0x"), 16).unwrap_or(0);
                let size = u64::from_str_radix(parts[1].trim_start_matches("0x"), 16).unwrap_or(0);
                if let Some(status) = parts[2].chars().next().and_then(BlockStatus::from_char) {
                    entries.push(MapEntry { pos, size, status });
                }
            }
        }

        Ok(Self {
            current_pos,
            current_pass,
            entries,
            file_path: path.to_path_buf(),
        })
    }

    pub fn save(&self) -> Result<()> {
        let mut file = fs::File::create(&self.file_path)?;
        writeln!(file, "# AegisDisk Rescue Mapfile")?;
        writeln!(file, "current_pos = {}", self.current_pos)?;
        writeln!(file, "current_pass = {}", self.current_pass)?;
        writeln!(file, "# pos        size       status")?;

        for e in &self.entries {
            writeln!(
                file,
                "0x{:012X}  0x{:08X}  {}",
                e.pos,
                e.size,
                e.status.to_char()
            )?;
        }

        file.flush()?;
        Ok(())
    }

    pub fn update_range(&mut self, pos: u64, size: u64, new_status: BlockStatus) {
        // Find intersecting entries and update/split
        let mut new_entries = Vec::with_capacity(self.entries.len() + 2);
        let target_end = pos + size;

        for e in self.entries.drain(..) {
            let e_end = e.pos + e.size;

            if e_end <= pos || e.pos >= target_end {
                // Outside range
                new_entries.push(e);
            } else {
                // Intersecting
                if e.pos < pos {
                    new_entries.push(MapEntry {
                        pos: e.pos,
                        size: pos - e.pos,
                        status: e.status,
                    });
                }

                let overlap_start = pos.max(e.pos);
                let overlap_end = target_end.min(e_end);
                new_entries.push(MapEntry {
                    pos: overlap_start,
                    size: overlap_end - overlap_start,
                    status: new_status,
                });

                if e_end > target_end {
                    new_entries.push(MapEntry {
                        pos: target_end,
                        size: e_end - target_end,
                        status: e.status,
                    });
                }
            }
        }

        self.entries = new_entries;
        self.current_pos = target_end;
    }

    pub fn stats(&self) -> (u64, u64, u64) {
        let mut rescued = 0u64;
        let mut bad = 0u64;
        let mut pending = 0u64;

        for e in &self.entries {
            match e.status {
                BlockStatus::Finished => rescued += e.size,
                BlockStatus::Bad => bad += e.size,
                _ => pending += e.size,
            }
        }

        (rescued, bad, pending)
    }

    /// Return the block status and remaining bytes within that entry from `pos`.
    pub fn entry_at(&self, pos: u64) -> Option<(BlockStatus, u64)> {
        for e in &self.entries {
            if pos >= e.pos && pos < e.pos + e.size {
                let remaining = (e.pos + e.size) - pos;
                return Some((e.status, remaining));
            }
        }
        None
    }
}
