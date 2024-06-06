use clap::Parser;

#[derive(Parser, Debug)]
#[command(version)]
pub struct Arguments {
    /// Torrent file to use
    #[arg(value_name = "TORRENT_FILE")]
    pub file: String,
    /// Where to save the downloaded file
    #[arg(short, long, value_name = "OUTPUT_FILE")]
    pub output: Option<String>,
}
