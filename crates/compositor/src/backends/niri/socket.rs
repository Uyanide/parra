use std::path::Path;

use crate::BackendError;
use crate::backends::lines::Lines;

/// Subscribes to the event stream. Sent verbatim, for the same reason the events are
/// modelled by hand: no dependency on the compositor's own request types.
const SUBSCRIBE: &[u8] = b"\"EventStream\"\n";

/// Connects and subscribes, failing if the compositor does not accept.
pub fn open(path: &Path, backend: &'static str) -> Result<Lines, BackendError> {
    let mut stream = Lines::connect(path, backend)?;
    stream.send(SUBSCRIBE)?;

    let reply = stream.expect_line()?;
    if !reply.contains("\"Ok\"") {
        return Err(stream.protocol(format!("refused the event stream: {}", reply.trim())));
    }
    Ok(stream)
}
