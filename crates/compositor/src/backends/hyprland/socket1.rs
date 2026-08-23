use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::BackendError;

/// Long enough for an answer, short enough that waiting for one that will never come is
/// not itself the outage.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Asks one question and hangs up.
///
/// Opened and closed around every request rather than kept, because the compositor serves
/// this socket strictly one connection at a time: a connection held open blocks every other
/// request to it, and the compositor's own handling of them, until its timeout elapses.
/// Measured at three seconds of a wedged compositor for a connection held silent for three,
/// against eight milliseconds for the same request unobstructed.
///
/// Which is the other half of why this is only ever asked at cold start and when a monitor
/// is left showing something no event stated. Nothing on the event path needs it, so no
/// burst of compositor activity can turn into a queue of requests here.
pub fn ask(socket: &Path, request: &str, backend: &'static str) -> Result<String, BackendError> {
    let io_error = |source| BackendError::Io { backend, source };

    let mut stream = UnixStream::connect(socket).map_err(io_error)?;
    stream.set_read_timeout(Some(TIMEOUT)).map_err(io_error)?;
    stream.set_write_timeout(Some(TIMEOUT)).map_err(io_error)?;
    stream.write_all(request.as_bytes()).map_err(io_error)?;
    stream.flush().map_err(io_error)?;

    // The compositor closes its end when the answer is complete, so end of stream is the
    // only framing there is.
    let mut answer = String::new();
    stream.read_to_string(&mut answer).map_err(io_error)?;
    Ok(answer)
}
