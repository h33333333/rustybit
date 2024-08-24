use std::{ops::Deref, path::Path};

use crate::{parser::Info, storage::FileInfo};

#[derive(Debug)]
pub struct TorrentFileMetadata {
    pub file_infos: Vec<FileInfo>,
}

impl TorrentFileMetadata {
    pub fn new(info: &mut Info, base_path: &Path) -> anyhow::Result<Self> {
        let mut file_infos = Vec::with_capacity(1);
        if let Some(files) = info.files.as_deref() {
            file_infos.reserve(files.len());
            for file in files.iter() {
                let mut path = base_path.to_path_buf();
                file.path.iter().for_each(|path_part| {
                    path.push(path_part.deref());
                });

                file_infos.push(FileInfo::new(path, file.length));
            }
        } else {
            let mut path = base_path.to_path_buf();
            path.push(&*info.name);

            let length = info.length.ok_or_else(|| {
                anyhow::anyhow!(
                    "Error while starting up a torrent: the `length` field is missing a single-file download mode",
                )
            })?;

            file_infos.push(FileInfo::new(path, length));
        };

        Ok(TorrentFileMetadata { file_infos })
    }
}
