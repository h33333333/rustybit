# RustyBit

A simple CLI bittorrent client.

It also partially supports the DHT protocol.

## Installation

```bash
cargo install rustybit
```

## Usage

To download a torrent into the `$(pwd)/.downloads` folder, run the following command:

```bash
rustybit <torrent_path>
```

If you want to change the output directory, use this command instead:

```bash
rustybit <torrent_path> -o <output_dir_path>
```

## Missing features

It currently misses the following features (and I'm not sure whether I would add them):

- Seeding torrents
- Extension protocol support
- Full DHT node
- Downloading multiple torrents in parallel
- Pausing/resuming torrents
- Proper GUI/TUI
