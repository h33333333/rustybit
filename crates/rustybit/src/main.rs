use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use rustybit_lib::parser;
use rustybit_lib::torrent::TorrentManager;
use tracing_subscriber::filter::{EnvFilter, LevelFilter};

// TODO: make DHT find peers faster?
const TRACING_ENV: &str = "BTT_LOG";

#[derive(Parser, Debug)]
#[command(version)]
struct Arguments {
    /// Torrent file to use
    #[arg(value_name = "TORRENT_FILE")]
    pub torrent: String,
    /// Where to save the downloaded torrents
    #[arg(short, long, value_name = "OUTPUT_DIR", default_value = Path::new("./downloads").to_path_buf().into_os_string())]
    pub output_dir: PathBuf,
}

#[tokio::main]
#[tracing::instrument(err)]
async fn main() -> anyhow::Result<()> {
    setup_logger();

    let args = Arguments::parse();

    let torrent_file = read_file(&args.torrent)?;
    let meta_info: parser::MetaInfo = serde_bencode::from_bytes(&torrent_file)?;

    let mut torrent_manager = TorrentManager::new(args.output_dir)?;
    let Some(root_task) = torrent_manager.add_new_torrent(meta_info).await? else {
        return Ok(());
    };

    root_task
        .await
        .context("failed to execute the root task")?
        .context("error while executing the root task")
}

fn read_file(path: &str) -> anyhow::Result<Vec<u8>> {
    let mut buffer = vec![];
    let mut file = fs::File::open(path)?;
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn setup_logger() {
    let env_filter = EnvFilter::builder()
        .with_env_var(TRACING_ENV)
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    let subscriber = tracing_subscriber::fmt().compact().with_env_filter(env_filter).finish();

    tracing::subscriber::set_global_default(subscriber).expect("Error setting a global tracing::subscriber");
}
