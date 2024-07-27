use std::path::Path;

use crate::{parser::Info, storage::FileInfo, Error, Result};

#[derive(Debug)]
pub struct TorrentFileMetadata {
    pub file_infos: Vec<FileInfo>,
}

impl TorrentFileMetadata {
    pub fn new(info: &mut Info, base_path: &Path) -> Result<Self> {
        let mut file_infos = Vec::with_capacity(1);
        if let Some(files) = info.files.as_deref_mut() {
            file_infos.reserve(files.len());
            for file in files.iter_mut() {
                let mut path = base_path.to_path_buf();
                file.path.iter().for_each(|path_part| {
                    path.push(path_part);
                });

                file_infos.push(FileInfo::new(path, file.length));
            }
        } else {
            let mut path = base_path.to_path_buf();
            path.push(&info.name);

            let length = info.length.ok_or_else(|| {
                Error::InternalError(
                    "Error while starting up a torrent: the `length` field is missing a single-file download mode",
                )
            })?;

            file_infos.push(FileInfo::new(path, length));
        };

        Ok(TorrentFileMetadata { file_infos })
    }
}
