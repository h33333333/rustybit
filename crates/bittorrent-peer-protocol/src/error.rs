use std::fmt::Debug;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("an IO error happened: {0}")]
    IoError(#[from] std::io::Error),
    #[error("an error happened when converting a value: {0}")]
    ConversionError(&'static str),
    #[error("error while converting bytes to a str: {0}")]
    Utf8Error(#[from] std::str::Utf8Error),
    #[error("expected at least {0} bytes while got {1}")]
    BadLength(usize, usize),
    #[error("error while converting str to a String: {0}")]
    FromUtf8Error(#[from] std::string::FromUtf8Error),
}
