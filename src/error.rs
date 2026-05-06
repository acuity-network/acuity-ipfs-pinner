use std::fmt;

#[derive(Debug)]
pub enum Error {
    WebSocket(tokio_tungstenite::tungstenite::Error),
    Http(reqwest::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    ProstDecode(prost::DecodeError),
    Protocol(String),
    InvalidIpfsHash(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebSocket(error) => write!(f, "websocket error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::ProstDecode(error) => write!(f, "protobuf decode error: {error}"),
            Self::Protocol(message) => write!(f, "protocol error: {message}"),
            Self::InvalidIpfsHash(message) => write!(f, "invalid ipfs hash: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(value: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(value)
    }
}

impl From<reqwest::Error> for Error {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<prost::DecodeError> for Error {
    fn from(value: prost::DecodeError) -> Self {
        Self::ProstDecode(value)
    }
}
