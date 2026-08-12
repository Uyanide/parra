use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::BackendError;

/// Subscribes to the event stream. Sent verbatim, for the same reason the events are
/// modelled by hand: no dependency on the compositor's own request types.
const SUBSCRIBE: &[u8] = b"\"EventStream\"\n";

/// How long a read waits before returning empty-handed. The shutdown check is the only
/// thing that needs it.
const POLL: Duration = Duration::from_secs(1);

pub struct Stream {
    reader: BufReader<UnixStream>,
    backend: &'static str,
}

impl Stream {
    /// Connects and subscribes, failing if the compositor does not accept.
    pub fn open(path: &Path, backend: &'static str) -> Result<Self, BackendError> {
        let io_error = |source| BackendError::Io { backend, source };
        let stream = UnixStream::connect(path).map_err(io_error)?;
        stream.set_read_timeout(Some(POLL)).map_err(io_error)?;
        (&stream).write_all(SUBSCRIBE).map_err(io_error)?;
        (&stream).flush().map_err(io_error)?;

        let mut reader = BufReader::new(stream);
        let mut reply = String::new();
        reader.read_line(&mut reply).map_err(io_error)?;
        if !reply.contains("\"Ok\"") {
            return Err(BackendError::Protocol {
                backend,
                message: format!("refused the event stream: {}", reply.trim()),
            });
        }
        Ok(Self { reader, backend })
    }

    /// One line, or `None` when the wait elapsed with nothing to read. An ended stream
    /// is reported as an error, because it means the compositor went away.
    pub fn next_line(&mut self) -> Result<Option<String>, BackendError> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Err(BackendError::Protocol {
                backend: self.backend,
                message: "the event stream ended".to_owned(),
            }),
            Ok(_) => Ok(Some(line)),
            Err(source) if timed_out(&source) => Ok(None),
            Err(source) => Err(BackendError::Io { backend: self.backend, source }),
        }
    }
}

fn timed_out(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}
