use anyhow::Context;
use bittorrent_client_rs::logging::setup_logger;
use bittorrent_client_rs::torrent::TorrentManager;
use bittorrent_client_rs::util::read_file;
use bittorrent_client_rs::{args, parser};
use clap::Parser;

// TODO: global code refactoring/restructuring
// TODO: slow speed build up. Can I improve it more?
// TODO: Improve DHT
//    - make it find peers faster
//    - send request not only to nodes but to peers?
// TODO: improve last pieces downloading speed

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
