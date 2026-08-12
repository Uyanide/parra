use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io, thread};

use thiserror::Error;
use tracing::{debug, warn};

use crate::protocol::{Request, Response};

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("another daemon is already listening on {}", path.display())]
    AlreadyRunning { path: PathBuf },
    #[error("cannot listen on {}", path.display())]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot start the control thread")]
    Spawn {
        #[source]
        source: io::Error,
    },
}

/// Turns a request into a reply, implemented by whoever owns the state.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: Request) -> Response;
}

/// The listening half of the control socket.
///
/// Owns the socket file and removes it when dropped, so a daemon that exits does not
/// leave a path behind that looks occupied.
pub struct Server {
    path: PathBuf,
    listener: UnixListener,
}

impl Server {
    /// Binds the socket, taking over a path left behind by a daemon that did not get to
    /// clean up. A path something still answers on is refused instead: two daemons
    /// sharing one socket would each receive an arbitrary half of the requests.
    pub fn bind(path: &Path) -> Result<Self, ServerError> {
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                return Err(ServerError::AlreadyRunning { path: path.to_owned() });
            }
            debug!(path = %path.display(), "taking over a socket nothing is listening on");
            let _ = fs::remove_file(path);
        }

        let listener = UnixListener::bind(path)
            .map_err(|source| ServerError::Bind { path: path.to_owned(), source })?;
        Ok(Self { path: path.to_owned(), listener })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Starts accepting connections on a thread of its own.
    ///
    /// The listener is duplicated rather than moved, so the socket file stays owned by
    /// this value and is removed when the daemon drops it.
    pub fn spawn<H: Handler>(&self, handler: H) -> Result<(), ServerError> {
        let listener = self.listener.try_clone().map_err(|source| ServerError::Spawn { source })?;
        let handler = Arc::new(handler);
        thread::Builder::new()
            .name("control".to_owned())
            .spawn(move || accept(&listener, &handler))
            .map_err(|source| ServerError::Spawn { source })?;
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn accept<H: Handler>(listener: &UnixListener, handler: &Arc<H>) {
    for connection in listener.incoming() {
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                warn!(%error, "the control socket stopped accepting");
                return;
            }
        };

        // One thread per connection, so a client that connects and says nothing holds up
        // nobody. Only something already running as this user can reach the socket.
        let handler = Arc::clone(handler);
        if let Err(error) = thread::Builder::new()
            .name("control-client".to_owned())
            .spawn(move || serve(stream, handler.as_ref()))
        {
            warn!(%error, "cannot answer a control connection");
        }
    }
}

/// Answers requests until the client goes away.
///
/// A line that is not a request is answered rather than fatal: a client that gets one
/// thing wrong can carry on, and only a closed connection ends the conversation.
fn serve<H: Handler>(stream: UnixStream, handler: &H) {
    let Ok(reading) = stream.try_clone() else { return };
    let mut writer = stream;

    for line in BufReader::new(reading).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handler.handle(request),
            Err(error) => Response::Error { message: error.to_string() },
        };

        let mut reply = serde_json::to_string(&response).unwrap_or_else(|error| {
            format!(r#"{{"error":{{"message":"cannot serialize the reply: {error}"}}}}"#)
        });
        reply.push('\n');
        if writer.write_all(reply.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::client::{Client, ClientError};
    use crate::protocol::PROTOCOL_VERSION;

    /// Answers everything the same way, which is all a socket test can check.
    struct Echo;

    impl Handler for Echo {
        fn handle(&self, request: Request) -> Response {
            match request {
                Request::Ping => Response::Pong { version: PROTOCOL_VERSION },
                other => Response::Error { message: format!("{other:?}") },
            }
        }
    }

    fn socket() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("control-{}-{unique}.sock", std::process::id()))
    }

    fn listening(path: &Path) -> Server {
        let server = Server::bind(path).expect("the socket should be free");
        server.spawn(Echo).expect("the control thread should start");
        server
    }

    #[test]
    fn a_request_comes_back_answered() {
        let path = socket();
        let _server = listening(&path);

        let mut client = Client::connect(&path).unwrap();
        assert_eq!(
            client.request(&Request::Ping).unwrap(),
            Response::Pong { version: PROTOCOL_VERSION }
        );
    }

    #[test]
    fn one_connection_can_ask_more_than_once() {
        let path = socket();
        let _server = listening(&path);

        let mut client = Client::connect(&path).unwrap();
        for _ in 0..3 {
            assert!(client.request(&Request::Ping).is_ok());
        }
    }

    #[test]
    fn a_line_that_is_not_a_request_is_answered_rather_than_fatal() {
        let path = socket();
        let _server = listening(&path);

        let mut raw = UnixStream::connect(&path).unwrap();
        raw.write_all(b"not json at all\n").unwrap();
        let mut reply = String::new();
        BufReader::new(raw.try_clone().unwrap()).read_line(&mut reply).unwrap();
        assert!(reply.contains("error"), "{reply}");

        let mut client = Client::connect(&path).unwrap();
        assert!(client.request(&Request::Ping).is_ok(), "the socket should still work");
    }

    #[test]
    fn a_socket_left_behind_by_a_dead_daemon_is_taken_over() {
        let path = socket();
        fs::write(&path, b"not a socket").unwrap();

        let _server = listening(&path);
        let mut client = Client::connect(&path).unwrap();
        assert!(client.request(&Request::Ping).is_ok());
    }

    #[test]
    fn a_second_daemon_refuses_to_share_the_socket() {
        let path = socket();
        let _server = listening(&path);

        let Err(error) = Server::bind(&path) else { panic!("a live socket must not be rebound") };
        assert!(matches!(error, ServerError::AlreadyRunning { .. }), "{error:?}");
    }

    #[test]
    fn dropping_the_server_takes_the_socket_file_with_it() {
        let path = socket();
        drop(listening(&path));

        assert!(!path.exists());
        let Err(error) = Client::connect(&path) else { panic!("nothing should be listening") };
        assert!(matches!(error, ClientError::NotRunning { .. }), "{error:?}");
    }
}
