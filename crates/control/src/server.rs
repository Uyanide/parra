use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::{fs, io, thread};

use thiserror::Error;
use tracing::{debug, warn};

use crate::protocol::{Event, Request, Response};

/// Events one subscriber may have waiting. Deep enough to absorb a burst, shallow enough
/// that a client which stopped reading is noticed rather than accumulated.
const BACKLOG: usize = 64;

/// How long a write to a subscriber may block before that connection is given up on.
/// Reached only by a client that neither reads nor closes, whose thread would otherwise
/// wait for ever.
const STALL: Duration = Duration::from_secs(5);

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

    /// Takes on a listener. Answering with [`Response::Error`] refuses it and leaves the
    /// connection answering requests.
    fn subscribe(&self, subscriber: Subscriber) -> Response;
}

/// The daemon's end of one subscribed connection.
///
/// Events are queued here rather than written, because the thread that knows about them is
/// the one that must never wait on a socket.
pub struct Subscriber {
    events: mpsc::SyncSender<Event>,
}

impl Subscriber {
    /// Queues one event, returning whether this subscriber is still worth keeping.
    ///
    /// A full queue drops the connection rather than the event: a client that far behind
    /// has stopped reading, and one that reconnects is described from scratch, where a
    /// silently missing line would leave it believing something stale.
    pub fn send(&self, event: &Event) -> bool {
        self.events.try_send(event.clone()).is_ok()
    }
}

/// Everyone listening, and the rule for dropping those who stopped.
#[derive(Default)]
pub struct Subscribers {
    listeners: Vec<Subscriber>,
}

impl Subscribers {
    pub fn add(&mut self, subscriber: Subscriber) {
        self.listeners.push(subscriber);
    }

    pub fn emit(&mut self, event: &Event) {
        self.listeners.retain(|listener| listener.send(event));
    }
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

/// A connection, which answers requests until it either ends or subscribes.
fn serve<H: Handler>(stream: UnixStream, handler: &H) {
    let Ok(reading) = stream.try_clone() else { return };
    let mut writer = stream;
    if let Some(events) = answer(BufReader::new(reading), &mut writer, handler) {
        push(writer, &events);
    }
}

/// Answers requests until the client goes away, returning the queue to push on when one of
/// them subscribed.
///
/// A line that is not a request is answered rather than fatal: a client that gets one
/// thing wrong can carry on, and only a closed connection ends the conversation.
fn answer<H: Handler>(
    reader: BufReader<UnixStream>,
    writer: &mut UnixStream,
    handler: &H,
) -> Option<mpsc::Receiver<Event>> {
    for line in reader.lines() {
        let Ok(line) = line else { return None };
        if line.trim().is_empty() {
            continue;
        }

        let (response, events) = respond(handler, &line);
        let reply = serde_json::to_string(&response).unwrap_or_else(|error| {
            format!(r#"{{"error":{{"message":"cannot serialize the reply: {error}"}}}}"#)
        });
        if !write_line(writer, reply) {
            return None;
        }
        if events.is_some() {
            return events;
        }
    }
    None
}

/// Answers one line. The queue comes back only for a subscription the daemon accepted,
/// since a refusal leaves the connection answering requests.
fn respond<H: Handler>(handler: &H, line: &str) -> (Response, Option<mpsc::Receiver<Event>>) {
    let request = match serde_json::from_str::<Request>(line) {
        Ok(request) => request,
        Err(error) => return (Response::Error { message: error.to_string() }, None),
    };
    if !matches!(request, Request::Subscribe) {
        return (handler.handle(request), None);
    }

    let (sender, events) = mpsc::sync_channel(BACKLOG);
    match handler.subscribe(Subscriber { events: sender }) {
        refusal @ Response::Error { .. } => (refusal, None),
        accepted => (accepted, Some(events)),
    }
}

/// Pushes events until the daemon stops sending or the client stops reading.
///
/// Whatever the client writes from here on is never read: every line going the other way
/// is an event now, and a reply among them could not be told apart from one.
fn push(mut writer: UnixStream, events: &mpsc::Receiver<Event>) {
    let _ = writer.set_write_timeout(Some(STALL));
    for event in events {
        // A line that cannot be serialized, which takes a path that is not UTF-8, ends the
        // connection for the same reason a full queue does: a listener is never left to
        // believe it has heard everything.
        let Ok(line) = serde_json::to_string(&event) else {
            warn!(?event, "cannot put this on the wire, so the listener is dropped");
            return;
        };
        if !write_line(&mut writer, line) {
            return;
        }
    }
}

/// Writes one JSON line, returning whether the connection can still be written to.
fn write_line(writer: &mut UnixStream, mut line: String) -> bool {
    line.push('\n');
    writer.write_all(line.as_bytes()).is_ok() && writer.flush().is_ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
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

        fn subscribe(&self, _: Subscriber) -> Response {
            Response::Error { message: "this one has no events".to_owned() }
        }
    }

    /// Keeps its listeners, so a test can push to them when it likes.
    #[derive(Clone, Default)]
    struct Talkative {
        subscribers: Arc<Mutex<Subscribers>>,
    }

    impl Talkative {
        fn emit(&self, event: &Event) {
            self.subscribers.lock().unwrap().emit(event);
        }
    }

    impl Handler for Talkative {
        fn handle(&self, _: Request) -> Response {
            Response::Pong { version: PROTOCOL_VERSION }
        }

        fn subscribe(&self, subscriber: Subscriber) -> Response {
            self.subscribers.lock().unwrap().add(subscriber);
            Response::Done
        }
    }

    fn socket() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("control-{}-{unique}.sock", std::process::id()))
    }

    /// A daemon of another build: it reports its own version and rejects what it cannot
    /// parse, which is what a wire format that denies unknown fields does on a skew.
    struct Stale;

    impl Handler for Stale {
        fn handle(&self, request: Request) -> Response {
            match request {
                Request::Ping => Response::Pong { version: PROTOCOL_VERSION + 1 },
                _ => Response::Error { message: "unknown field `save`".to_owned() },
            }
        }

        fn subscribe(&self, _: Subscriber) -> Response {
            Response::Error { message: "unknown variant `subscribe`".to_owned() }
        }
    }

    fn listening(path: &Path) -> Server {
        serving(path, Echo)
    }

    fn serving<H: Handler>(path: &Path, handler: H) -> Server {
        let server = Server::bind(path).expect("the socket should be free");
        server.spawn(handler).expect("the control thread should start");
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
    fn a_refusal_by_a_daemon_of_another_version_names_the_skew() {
        let path = socket();
        let _server = serving(&path, Stale);

        let mut client = Client::connect(&path).unwrap();
        let Err(error) = client.request(&Request::ReloadConfig) else {
            panic!("a stale daemon must not accept this")
        };
        assert!(
            matches!(error, ClientError::Mismatch { daemon, ours }
                if daemon == PROTOCOL_VERSION + 1 && ours == PROTOCOL_VERSION),
            "{error:?}"
        );
    }

    #[test]
    fn a_refusal_by_a_daemon_of_this_version_stays_a_refusal() {
        let path = socket();
        let _server = listening(&path);

        let Err(error) = Client::connect(&path).unwrap().request(&Request::ReloadConfig) else {
            panic!("the echo handler refuses everything but a ping")
        };
        assert!(matches!(error, ClientError::Refused { .. }), "{error:?}");
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

    /// Subscribes on a raw connection, leaving the reader positioned after the reply, for
    /// the tests that need to misbehave in ways a `Client` will not.
    fn raw_subscription(path: &Path) -> (UnixStream, BufReader<UnixStream>) {
        let stream = UnixStream::connect(path).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        writer.write_all(b"\"subscribe\"\n").unwrap();

        let mut reply = String::new();
        reader.read_line(&mut reply).unwrap();
        assert_eq!(reply.trim(), "\"done\"", "the subscription should have been accepted");
        (writer, reader)
    }

    #[test]
    fn a_subscriber_is_sent_what_the_daemon_pushes() {
        let path = socket();
        let handler = Talkative::default();
        let _server = serving(&path, handler.clone());

        let mut events = Client::connect(&path).unwrap().subscribe().unwrap();
        handler.emit(&Event::ConfigReloaded);
        assert_eq!(events.next().unwrap().unwrap(), Event::ConfigReloaded);
    }

    #[test]
    fn a_subscribed_connection_stops_answering() {
        let path = socket();
        let handler = Talkative::default();
        let _server = serving(&path, handler.clone());
        let (mut writer, mut reader) = raw_subscription(&path);

        // A reply to this could not be told apart from an event, so there must not be one.
        writer.write_all(b"\"ping\"\n").unwrap();
        handler.emit(&Event::ConfigReloaded);

        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Event>(&line).unwrap(),
            Event::ConfigReloaded,
            "a pong ahead of the event would mean the connection was still answering"
        );
    }

    #[test]
    fn a_refused_subscription_leaves_the_connection_answering() {
        let path = socket();
        let _server = listening(&path);

        let mut client = Client::connect(&path).unwrap();
        assert!(matches!(client.request(&Request::Subscribe), Err(ClientError::Refused { .. })));
        assert!(client.request(&Request::Ping).is_ok(), "the connection should still work");
    }

    #[test]
    fn a_subscriber_that_stops_reading_is_dropped_rather_than_waited_on() {
        const PUSHED: usize = 100_000;

        let path = socket();
        let handler = Talkative::default();
        let _server = serving(&path, handler.clone());
        let (_writer, mut reader) = raw_subscription(&path);

        // Nothing reads while this runs. The test hanging here would itself be the
        // failure: the daemon's end must never wait on a socket.
        for _ in 0..PUSHED {
            handler.emit(&Event::ConfigReloaded);
        }

        // What the socket buffered arrives, and then the connection is simply gone.
        let mut received = 0;
        let mut line = String::new();
        while reader.read_line(&mut line).unwrap() > 0 {
            line.clear();
            received += 1;
        }
        assert!(received < PUSHED, "a listener this far behind is dropped, not queued for ever");
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
