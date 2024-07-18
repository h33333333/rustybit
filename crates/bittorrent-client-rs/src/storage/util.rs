use anyhow::Context;

use super::FileInfo;

pub(crate) struct FileOffsets {
    pub file_idx: usize,
    pub offset_into_file: u64,
    pub lfile_bytes: u64,
    pub rfile_bytes: u64,
}

pub(crate) fn find_file_offsets_for_data(
    file_infos: &[FileInfo],
    piece_idx: u32,
    piece_length: u64,
    data_length: usize,
    begin: Option<u32>,
) -> anyhow::Result<Option<FileOffsets>> {
    let global_data_offset = (piece_idx as u64 * piece_length) + begin.unwrap_or(0) as u64;
    let data_length = try_into!(data_length, u64)?;

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
                // Check if we cross a file boundary
                let rfile_bytes = if global_data_offset + data_length < *state {
                    // We don't cross the file boundary
                    0
                } else {
                    // We cross the file boundary and have to do two writes
                    global_data_offset + data_length - *state
                };

                let offset_into_file = global_data_offset - prev_length;

                Some(Some(FileOffsets {
                    file_idx,
                    offset_into_file,
                    lfile_bytes: data_length - rfile_bytes,
                    rfile_bytes,
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
