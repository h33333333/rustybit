use std::path::{Path, PathBuf};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version)]
pub struct Arguments {
    /// Torrent file to use
    #[arg(value_name = "TORRENT_FILE")]
    pub torrent: String,
    /// Where to save the downloaded torrents
    #[arg(short, long, value_name = "OUTPUT_DIR", default_value = Path::new("./downloads").to_path_buf().into_os_string())]
    pub output_dir: PathBuf,
}
