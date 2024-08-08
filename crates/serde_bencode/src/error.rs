#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub position: Option<usize>,
}

impl Error {
    pub fn set_position(mut self, position: usize) -> Self {
        self.position = Some(position);
        self
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(pos) = self.position {
            write!(f, "Error at position {}: {}", pos, self.kind)
        } else {
            write!(f, "Error: {}", self.kind)
        }
    }
}

impl std::error::Error for Error {}

impl serde::de::Error for Error {
    fn custom<T>(msg: T) -> Self
    where
        T: std::fmt::Display,
    {
        ErrorKind::Custom(msg.to_string()).into()
    }
}

impl serde::ser::Error for Error {
    fn custom<T>(msg: T) -> Self
    where
        T: std::fmt::Display,
    {
        ErrorKind::Custom(msg.to_string()).into()
    }
}

impl From<ErrorKind> for Error {
    fn from(value: ErrorKind) -> Self {
        Error {
            kind: value,
            position: Default::default(),
        }
    }
}

#[derive(Debug)]
pub enum ErrorKind {
    Custom(String),
    UnexpectedEof(&'static str),
    BadInputData(&'static str),
    Unsupported(&'static str),
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::Custom(msg) => write!(f, "{}", msg),
            ErrorKind::UnexpectedEof(expected) => {
                write!(f, "Unexpected EOF encounetered, expected \"{}\" instead", expected)
            }
            ErrorKind::BadInputData(msg) => write!(f, "Input data is broken: {}", msg),
            ErrorKind::Unsupported(msg) => write!(f, "Bencode doesn't support {}", msg),
        }
    }
}
