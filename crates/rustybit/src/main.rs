use anyhow::Context;
use clap::Parser;
use rustybit::logging::setup_logger;
use rustybit::torrent::TorrentManager;
use rustybit::util::read_file;
use rustybit::{args, parser};

// TODO: make DHT find peers faster?

#[tokio::main]
#[tracing::instrument(err)]
async fn main() -> anyhow::Result<()> {
    setup_logger();

    let args = args::Arguments::parse();

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
