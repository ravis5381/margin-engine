use crate::DataError;
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZipLimits {
    pub max_entries: usize,
    pub max_total_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
}

impl ZipLimits {
    pub const fn conservative(max_total_uncompressed_bytes: u64) -> Self {
        Self {
            max_entries: 64,
            max_total_uncompressed_bytes,
            max_compression_ratio: 100,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub bytes: Vec<u8>,
}

pub fn entry_name_allowed(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('/')
        && !name.starts_with('\\')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name
            .split('/')
            .any(|segment| segment == ".." || segment.is_empty() && name.contains('/'))
        && !name.contains("../")
        && name != ".."
}

/// Reads a ZIP archive into memory. It does not interpret any exchange file layout.
pub fn extract_zip_to_memory(input: &[u8], limits: ZipLimits) -> Result<Vec<ZipEntry>, DataError> {
    if limits.max_entries == 0 || limits.max_compression_ratio == 0 {
        return Err(DataError::ArchiveRejected {
            detail: "zip limits must be positive".to_owned(),
        });
    }
    let mut archive =
        ZipArchive::new(Cursor::new(input)).map_err(|error| DataError::ArchiveRejected {
            detail: format!("zip archive could not be opened: {error}"),
        })?;
    if archive.len() > limits.max_entries {
        return Err(DataError::ArchiveRejected {
            detail: format!(
                "zip contains {} entries, above the limit of {}",
                archive.len(),
                limits.max_entries
            ),
        });
    }
    let mut total = 0_u64;
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|error| DataError::ArchiveRejected {
                detail: format!("zip entry {index} could not be read: {error}"),
            })?;
        let Some(path) = file.enclosed_name() else {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry {index} has an unsafe path"),
            });
        };
        let Some(name) = path.to_str() else {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry {index} name is not UTF-8"),
            });
        };
        let name = name.to_owned();
        if !entry_name_allowed(&name) {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry `{name}` is not an allowed relative file name"),
            });
        }
        if file.is_dir() {
            continue;
        }
        if file.encrypted() {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry `{name}` is encrypted"),
            });
        }
        match file.compression() {
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated => {}
            other => {
                return Err(DataError::ArchiveRejected {
                    detail: format!("zip entry `{name}` uses unsupported compression {other:?}"),
                });
            }
        }
        if let Some(mode) = file.unix_mode() {
            const S_IFMT: u32 = 0o170000;
            const S_IFLNK: u32 = 0o120000;
            if mode & S_IFMT == S_IFLNK {
                return Err(DataError::ArchiveRejected {
                    detail: format!("zip entry `{name}` is a symbolic link"),
                });
            }
        }
        let declared = file.size();
        let compressed = file.compressed_size();
        if compressed == 0 && declared > 0 {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry `{name}` declares a decompression bomb"),
            });
        }
        if compressed > 0
            && u128::from(declared)
                > u128::from(compressed).saturating_mul(u128::from(limits.max_compression_ratio))
        {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry `{name}` exceeds the compression ratio limit"),
            });
        }
        if total.saturating_add(declared) > limits.max_total_uncompressed_bytes {
            return Err(DataError::ArchiveRejected {
                detail: "zip uncompressed size exceeds the configured limit".to_owned(),
            });
        }
        let remaining = limits.max_total_uncompressed_bytes - total;
        let mut buffer = Vec::new();
        file.take(remaining + 1)
            .read_to_end(&mut buffer)
            .map_err(|error| DataError::ArchiveRejected {
                detail: format!("zip entry `{name}` could not be read: {error}"),
            })?;
        if buffer.len() as u64 > remaining {
            return Err(DataError::ArchiveRejected {
                detail: format!("zip entry `{name}` exceeds the uncompressed byte budget"),
            });
        }
        total += buffer.len() as u64;
        entries.push(ZipEntry {
            name,
            bytes: buffer,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{ZipLimits, entry_name_allowed, extract_zip_to_memory};
    use std::io::{Cursor, Write};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    #[test]
    fn extracts_a_synthetic_zip_and_rejects_unsafe_names_and_oversize_output() {
        assert!(entry_name_allowed("synthetic.csv"));
        assert!(!entry_name_allowed("../secret.csv"));
        assert!(!entry_name_allowed("/etc/passwd"));
        assert!(!entry_name_allowed(".."));

        let bytes = zip_with(b"synthetic-not-official");
        let entries = extract_zip_to_memory(&bytes, ZipLimits::conservative(1024)).expect("zip");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "synthetic.csv");
        assert_eq!(entries[0].bytes, b"synthetic-not-official");

        let too_small = extract_zip_to_memory(
            &bytes,
            ZipLimits {
                max_entries: 4,
                max_total_uncompressed_bytes: 4,
                max_compression_ratio: 100,
            },
        );
        assert!(too_small.is_err());
    }

    fn zip_with(contents: &[u8]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            writer
                .start_file("synthetic.csv", SimpleFileOptions::default())
                .expect("start");
            writer.write_all(contents).expect("write");
            writer.finish().expect("finish");
        }
        cursor.into_inner()
    }
}
