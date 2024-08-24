use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;

use anyhow::Context;

use super::{FileInfo, Storage};

pub struct FileStorage {
    files: Vec<(PathBuf, File)>,
}

impl FileStorage {
    pub fn new(paths: &[FileInfo]) -> anyhow::Result<Self> {
        let mut files = Vec::with_capacity(paths.len());
        for file_info in paths.iter() {
            std::fs::create_dir_all(
                file_info
                    .path
                    .parent()
                    .with_context(|| format!("bug: a file with no parrent? {:?}", file_info.path))?,
            )
            .with_context(|| {
                format!(
                    "error while creating parent directories for a file: {:?}",
                    file_info.path
                )
            })?;
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .read(true)
                .open(&file_info.path)
                .with_context(|| format!("error while opening/creating a file: {:?}", file_info.path))?;

            f.set_len(file_info.length).with_context(|| {
                format!(
                    "error while setting the file's length: {:?}, {}",
                    file_info.path, file_info.length
                )
            })?;

            files.push((file_info.path.clone(), f));
        }
        Ok(FileStorage { files })
    }
}

impl Storage for FileStorage {
    #[tracing::instrument(err, skip(self, buf))]
    fn write_all(&mut self, file_idx: usize, offset: u64, buf: &[u8]) -> anyhow::Result<()> {
        let file = &mut self
            .files
            .get(file_idx)
            .map(|(_, file)| file)
            .context("bug: non-existing file index?")?;
        file.write_all_at(buf, offset)
            .context("error while writing to the provided offset")?;

        Ok(())
    }

    #[tracing::instrument(err, skip(self, buf))]
    fn read_exact(&mut self, file_idx: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<bool> {
        let file = &mut self
            .files
            .get(file_idx)
            .map(|(_, file)| file)
            .context("bug: non-existing file index?")?;
        if let Err(e) = file.read_exact_at(buf, offset) {
            match e.kind() {
                ErrorKind::UnexpectedEof => return Ok(false),
                _ => return Err(e).context("error while reading from file at the offset"),
            }
        };

        Ok(true)
    }
}
