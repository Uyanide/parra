use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use thiserror::Error;

use crate::protocol::{Request, Response};

/// Long enough that a busy daemon is never mistaken for a hung one, short enough that a
/// script does not wedge.
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("no daemon is listening on {}", path.display())]
    NotRunning { path: PathBuf },
    #[error("cannot reach the daemon on {}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the daemon closed the connection without replying")]
    Closed,
    #[error("the daemon sent a malformed reply")]
    Malformed {
        #[source]
        source: serde_json::Error,
    },
    #[error("{message}")]
    Refused { message: String },
}

/// One connection to the daemon, one request at a time.
///
/// Every subcommand except `daemon` is one round trip on this and nothing else, which is
/// why they start without touching a graphics stack.
pub struct Client {
    path: PathBuf,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(|source| match source.kind() {
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                ClientError::NotRunning { path: path.to_owned() }
            }
            _ => ClientError::Io { path: path.to_owned(), source },
        })?;
        let io_error = |source| ClientError::Io { path: path.to_owned(), source };
        stream.set_read_timeout(Some(TIMEOUT)).map_err(io_error)?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(io_error)?;
        Ok(Self { path: path.to_owned(), reader: BufReader::new(stream) })
    }

    /// Sends one request and returns the daemon's reply. An `Error` reply becomes
    /// [`ClientError::Refused`], so callers only handle failure in one place.
    pub fn request(&mut self, request: &Request) -> Result<Response, ClientError> {
        let mut line = serde_json::to_string(request).expect("requests always serialize");
        line.push('\n');

        let mut stream = self.reader.get_ref();
        stream.write_all(line.as_bytes()).map_err(|source| self.io(source))?;
        stream.flush().map_err(|source| self.io(source))?;

        let mut reply = String::new();
        let read = self.reader.read_line(&mut reply).map_err(|source| self.io(source))?;
        if read == 0 {
            return Err(ClientError::Closed);
        }

        match serde_json::from_str(&reply) {
            Ok(Response::Error { message }) => Err(ClientError::Refused { message }),
            Ok(response) => Ok(response),
            Err(source) => Err(ClientError::Malformed { source }),
        }
    }

    fn io(&self, source: io::Error) -> ClientError {
        ClientError::Io { path: self.path.clone(), source }
    }
}
