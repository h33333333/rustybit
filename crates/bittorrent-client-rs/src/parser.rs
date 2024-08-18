use core::str;
use std::borrow::Cow;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Multiple File Mode info
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct File<'a> {
    /// Length of the file in bytes
    pub length: u64,
    /// MD5 sum of the file
    #[serde(default, skip_serializing_if = "Option::is_none", borrow)]
    pub md5sum: Option<Cow<'a, str>>,
    /// A list containing one or more string elements that together represent the path and filename.
    /// Each element in the list corresponds to either a directory name or the filename.
    /// "dir1/dir2/file.ext" -> ["dir1", "dir2", "file.ext"]
    #[serde(borrow)]
    pub path: Vec<Cow<'a, str>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Info<'a> {
    /// A list of Files (Multi File Mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<File<'a>>>,
    /// Length of the file in bytes (Single File Mode)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    /// Filename (Single File Mode) / Name of the directory (Multi File Mode)
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    /// Number of bytes in each piece
    #[serde(rename = "piece length")]
    pub piece_length: usize,
    /// Concatenated piece hashes (20-byte SHA1 hash values). Must be a multiple of 20
    #[serde(with = "serde_bytes", borrow)]
    pub pieces: Cow<'a, [u8]>,
    /// External peer source (Can be either 0 or 1)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<u8>,
    /// MD5 sum of the file (Single File Mode)
    #[serde(default, skip_serializing_if = "Option::is_none", borrow)]
    pub md5sum: Option<Cow<'a, str>>,
}

impl<'a> Info<'a> {
    /// SHA1 Hash of bencoded self
    pub fn hash(&self) -> anyhow::Result<[u8; 20]> {
        let mut hasher = crypto_hash::Hasher::new(crypto_hash::Algorithm::SHA1);
        serde_bencode::to_writer(self, &mut hasher).context("serializing torrent's Info failed")?;
        let mut resulting_hash = [0u8; 20];
        resulting_hash.copy_from_slice(&hasher.finish());
        Ok(resulting_hash)
    }
}

#[derive(Debug, Deserialize)]
pub struct MetaInfo<'a> {
    /// Description of the file(s) of the torrent
    #[serde(borrow)]
    pub info: Info<'a>,
    /// The announce URL of the tracker
    #[serde(borrow)]
    pub announce: Cow<'a, str>,
    /// The string encoding that is used in the info.pieces
    #[serde(default, borrow)]
    pub encoding: Option<Cow<'a, str>>,
    /// A list of annouce URLs of trackers. Is used if the multitracker specification is supported
    #[serde(default)]
    #[serde(rename = "announce-list", borrow)]
    pub announce_list: Option<Vec<Vec<Cow<'a, str>>>>,
    /// The creation time of the torrent (UNIX epoch format)
    #[serde(default)]
    #[serde(rename = "creation date")]
    pub creation_date: Option<u64>,
    /// Free-form comments of the author
    #[serde(default, borrow)]
    pub comment: Option<Cow<'a, str>>,
    /// Name and version of the program used to create the Metainfo file
    #[serde(default)]
    #[serde(rename = "created by", borrow)]
    pub created_by: Option<Cow<'a, str>>,
}
