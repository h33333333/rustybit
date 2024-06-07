use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_bencode::ser;
use serde_with::{serde_as, Bytes};
use sha1::{Digest, Sha1};

/// Multiple File Mode info
#[derive(Debug, Serialize, Deserialize)]
pub struct File {
    /// A list containing one or more string elements that together represent the path and filename.
    /// Each element in the list corresponds to either a directory name or the filename.
    /// "dir1/dir2/file.ext" -> ["dir1", "dir2", "file.ext"]
    pub path: Vec<String>,
    /// Length of the file in bytes
    pub length: u64,
    /// MD5 sum of the file
    #[serde(default)]
    pub md5sum: Option<String>,
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct Info {
    /// Filename (Single File Mode) / Name of the directory (Multi File Mode)
    pub name: String,
    /// Concatenated piece hashes (20-byte SHA1 hash values). Must be a multiple of 20
    #[serde_as(as = "Bytes")]
    pub pieces: Vec<u8>,
    /// Number of bytes in each piece
    #[serde(rename = "piece length")]
    pub piece_length: u64,
    /// MD5 sum of the file (Single File Mode)
    #[serde(default)]
    pub md5sum: Option<String>,
    /// Length of the file in bytes (Single File Mode)
    #[serde(default)]
    pub length: Option<u64>,
    /// A list of Files (Multi File Mode)
    #[serde(default)]
    pub files: Option<Vec<File>>,
    /// External peer source (Can be either 0 or 1)
    #[serde(default)]
    pub private: Option<u8>,
}

impl Info {
    /// SHA1 Hash of bencoded self
    pub fn hash(&self) -> anyhow::Result<[u8; 20]> {
        let serialized_struct = ser::to_bytes(self).context("serializing torrent's Info failed")?;
        let hasher = Sha1::new_with_prefix(serialized_struct);
        Ok(hasher.finalize().into())
    }
}

#[derive(Debug, Deserialize)]
pub struct MetaInfo {
    /// Description of the file(s) of the torrent
    pub info: Info,
    /// The announce URL of the tracker
    pub announce: String,
    /// The string encoding that is used in the info.pieces
    #[serde(default)]
    pub encoding: Option<String>,
    /// A list of annouce URLs of trackers. Is used if the multitracker specification is supported
    #[serde(default)]
    #[serde(rename = "announce-list")]
    pub announce_list: Option<Vec<Vec<String>>>,
    /// The creation time of the torrent (UNIX epoch format)
    #[serde(default)]
    #[serde(rename = "creation date")]
    pub creation_date: Option<u64>,
    /// Free-form comments of the author
    #[serde(default)]
    pub comment: Option<String>,
    /// Name and version of the program used to create the Metainfo file
    #[serde(default)]
    #[serde(rename = "created by")]
    pub created_by: Option<String>,
}
