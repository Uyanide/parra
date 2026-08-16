use std::sync::Mutex;
use std::sync::mpsc;

use calloop::channel;
use control::{Handler, Request, Response, Subscriber};

/// What a connection wants from the event loop.
pub enum Ask {
    Answer(Request),
    /// To be sent events from here on, rather than answered.
    Listen(Subscriber),
}

/// One ask on its way to the event loop, carrying the line back to the client that is
/// waiting on it.
pub struct Call {
    pub ask: Ask,
    pub reply: mpsc::Sender<Response>,
}

/// The control socket's end of the event loop.
///
/// Connections are answered on threads of their own, and the daemon's state belongs to
/// the loop's thread alone, so a request crosses over and its answer crosses back. That
/// is what keeps a socket read from stalling a frame.
pub struct Bridge {
    // calloop's sender is not `Sync`, and every connection thread shares this one.
    calls: Mutex<channel::Sender<Call>>,
}

impl Bridge {
    pub fn new(calls: channel::Sender<Call>) -> Self {
        Self { calls: Mutex::new(calls) }
    }

    fn call(&self, ask: Ask) -> Response {
        let (reply, answer) = mpsc::channel();
        let sent = match self.calls.lock() {
            Ok(calls) => calls.send(Call { ask, reply }).is_ok(),
            Err(_) => false,
        };
        if !sent {
            return stopping();
        }

        // A dropped reply channel means the loop ended before answering. The client is
        // told so rather than left waiting for its own timeout.
        answer.recv().unwrap_or_else(|_| stopping())
    }
}

impl Handler for Bridge {
    fn handle(&self, request: Request) -> Response {
        self.call(Ask::Answer(request))
    }

    fn subscribe(&self, subscriber: Subscriber) -> Response {
        self.call(Ask::Listen(subscriber))
    }
}

fn stopping() -> Response {
    Response::Error { message: "the daemon is shutting down".to_owned() }
}
