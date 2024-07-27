use anyhow::Context;

use super::{FileInfo, Storage};

pub(crate) struct FileOffsets {
    pub file_idx: usize,
    pub offset_into_file: u64,
}

pub(crate) fn find_file_offsets_for_data(
    file_infos: &[FileInfo],
    piece_idx: u32,
    piece_length: u64,
    begin: Option<u32>,
) -> anyhow::Result<Option<FileOffsets>> {
    let global_data_offset = (piece_idx as u64 * piece_length) + begin.unwrap_or(0) as u64;
    file_infos
        .iter()
        .enumerate()
        .scan(0, |state, (file_idx, file_info)| {
            // We need this to calculate an in-file offset
            let prev_length = *state;

            // Update the length
            *state += file_info.length;

            // Look for a matching file
            if global_data_offset < *state {
                let offset_into_file = global_data_offset - prev_length;
                Some(Some(FileOffsets {
                    file_idx,
                    offset_into_file,
                }))
            } else {
                Some(None)
            }
        })
        .find(|el| el.is_some())
        // We use nested Options to make scan traverse the collection as long as we need
        // instead of stopping at the first None. Thus, the first Option is always going to be
        // Some.
        .context("bug: scan returned None?")
}

pub(crate) fn write_data_to_files(
    storage: &mut dyn Storage,
    buf: &[u8],
    file_offsets: FileOffsets,
    file_infos: &[FileInfo],
) -> anyhow::Result<bool> {
    let mut file_idx = file_offsets.file_idx;
    let mut bytes_to_write = buf.len();
    while bytes_to_write != 0 {
        let offset = if file_idx == file_offsets.file_idx {
            file_offsets.offset_into_file
        } else {
            0
        };
        let file = &file_infos.get(file_idx).context("bug: bad file idx")?;
        let current_file_bytes_to_write = bytes_to_write.min(try_into!(file.length - offset, usize)?);

        let written_bytes = buf.len() - bytes_to_write;
        storage
            .write_all(
                file_idx,
                offset,
                &buf[written_bytes..written_bytes + current_file_bytes_to_write],
            )
            .with_context(|| {
                format!(
                    "Error while reading piece from file: idx {}, offset {}",
                    file_idx, offset
                )
            })?;
        file_idx += 1;
        bytes_to_write -= current_file_bytes_to_write;
    }
    Ok(true)
}

pub(crate) fn read_data_from_files(
    storage: &mut dyn Storage,
    buf: &mut [u8],
    file_offsets: FileOffsets,
    file_infos: &[FileInfo],
    piece_length: usize,
) -> anyhow::Result<bool> {
    let mut file_idx = file_offsets.file_idx;
    let mut bytes_to_read = piece_length;
    while bytes_to_read != 0 {
        let offset = if file_idx == file_offsets.file_idx {
            file_offsets.offset_into_file
        } else {
            0
        };
        let file = &file_infos.get(file_idx).context("bug: bad file idx")?;
        let current_file_bytes_to_read = bytes_to_read.min(try_into!(file.length - offset, usize)?);

        let read_bytes = piece_length - bytes_to_read;
        let file_had_enough_bytes = storage
            .read_exact(
                file_idx,
                offset,
                &mut buf[read_bytes..read_bytes + current_file_bytes_to_read],
            )
            .with_context(|| {
                format!(
                    "Error while reading piece from file: idx {}, offset {}",
                    file_idx, offset
                )
            })?;
        if !file_had_enough_bytes {
            // We can exit early, as we will still have to redownload the whole piece
            return Ok(false);
        }
        file_idx += 1;
        bytes_to_read -= current_file_bytes_to_read;
    }
    Ok(true)
}
