use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::Context;

use super::{AsyncReadSeek, Storage};

pub struct FileStorage {
    files: Vec<(PathBuf, File)>,
}

impl FileStorage {
    pub fn new(paths: &[(&Path, u64)]) -> anyhow::Result<Self> {
        let mut files = Vec::with_capacity(paths.len());
        for (path, file_len) in paths.iter() {
            std::fs::create_dir_all(
                path.parent()
                    .with_context(|| format!("bug: a file with no parrent? {:?}", path))?,
            )
            .with_context(|| format!("error while creating parent directories for a file: {:?}", path))?;
            let f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .read(true)
                .open(path)
                .with_context(|| format!("error while opening/creating a file: {path:?}"))?;

            f.set_len(*file_len)
                .with_context(|| format!("error while setting the file's length: {path:?}, {file_len}"))?;

            files.push((path.to_path_buf(), f));
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
        file.seek(SeekFrom::Start(offset))
            .context("error while seeking the provided offset")?;
        file.write_all(buf).context("error while writing to file")?;

        Ok(())
    }

    #[tracing::instrument(err, skip(self, buf))]
    fn read_exact(&mut self, file_idx: usize, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
        let file = &mut self
            .files
            .get(file_idx)
            .map(|(_, file)| file)
            .context("bug: non-existing file index?")?;
        file.seek(SeekFrom::Start(offset))
            .context("error while seeking the provided offset")?;
        file.read_exact(buf).context("error while reading from file")?;

        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    fn get_ro_file(&self, file_idx: usize) -> anyhow::Result<Pin<Box<dyn AsyncReadSeek + Send>>> {
        let file_path = self
            .files
            .get(file_idx)
            .map(|(path, _)| path)
            .context("bug: non-existing file index?")?;

        let file = tokio::fs::File::from_std(
            std::fs::OpenOptions::new()
                .read(true)
                .open(file_path)
                .context("error while opening a RO file")?,
        );

        Ok(Box::pin(file) as Pin<Box<dyn AsyncReadSeek + Send>>)
    }
}
