//! Reading a line-oriented socket, which is how every compositor here reports what it did.

use std::io::{self, BufRead, BufReader, Write};
use std::mem;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::BackendError;

/// How long a read waits before returning empty-handed. The shutdown check is the only
/// thing that needs it, so it wants to be short enough to stop promptly and long enough to
/// cost nothing while idle.
const POLL: Duration = Duration::from_secs(1);

/// One connection, read a line at a time.
pub struct Lines {
    reader: BufReader<UnixStream>,
    backend: &'static str,
    /// The line so far, when a read gave up part way through one. Held rather than
    /// dropped: those bytes are already off the socket, and losing them would take the
    /// start of the line with them and split every line after it in the wrong place.
    partial: String,
}

impl Lines {
    pub fn connect(path: &Path, backend: &'static str) -> Result<Self, BackendError> {
        let io_error = |source| BackendError::Io { backend, source };
        let stream = UnixStream::connect(path).map_err(io_error)?;
        stream.set_read_timeout(Some(POLL)).map_err(io_error)?;
        Ok(Self { reader: BufReader::new(stream), backend, partial: String::new() })
    }

    /// Says something before listening, for a protocol that has to ask first.
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), BackendError> {
        let mut stream = self.reader.get_ref();
        let io_error = |source| BackendError::Io { backend: self.backend, source };
        stream.write_all(bytes).map_err(io_error)?;
        stream.flush().map_err(io_error)
    }

    /// One line, or `None` when the wait elapsed with nothing to read. An ended stream is
    /// reported as an error, because it means the compositor went away.
    pub fn next_line(&mut self) -> Result<Option<String>, BackendError> {
        match self.reader.read_line(&mut self.partial) {
            Ok(0) => Err(self.protocol("the event stream ended")),
            Ok(_) => Ok(Some(mem::take(&mut self.partial))),
            Err(source) if timed_out(&source) => Ok(None),
            Err(source) => Err(BackendError::Io { backend: self.backend, source }),
        }
    }

    /// The next line, where silence is itself a failure. For a reply that was asked for
    /// and so must arrive.
    pub fn expect_line(&mut self) -> Result<String, BackendError> {
        self.next_line()?.ok_or_else(|| self.protocol("no answer"))
    }

    pub fn protocol(&self, message: impl Into<String>) -> BackendError {
        BackendError::Protocol { backend: self.backend, message: message.into() }
    }
}

fn timed_out(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read gives up on the shutdown poll wherever it happens to be, which can be part
    /// way through a line. What it took by then is off the socket for good.
    #[test]
    fn a_line_the_wait_cut_in_half_still_arrives_whole() {
        let (mut writer, reader) = UnixStream::pair().expect("a pair of connected sockets");
        reader.set_read_timeout(Some(Duration::from_millis(10))).expect("a read timeout");
        let mut lines =
            Lines { reader: BufReader::new(reader), backend: "test", partial: String::new() };

        writer.write_all(b"the first").expect("the start of a line");
        assert_eq!(lines.next_line().expect("a wait, not a failure"), None, "no line ended yet");

        writer.write_all(b" half\nthe next line\n").expect("the rest of it");
        let read = |lines: &mut Lines| lines.next_line().expect("a line").expect("a line");
        assert_eq!(read(&mut lines), "the first half\n", "the halves belong to one line");
        assert_eq!(read(&mut lines), "the next line\n", "and the line after it is untouched");
    }
}
