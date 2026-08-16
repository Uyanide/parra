use std::os::fd::AsFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use anyhow::Context as _;
use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction, channel, generic::Generic};
use compositor::{CompositorEvent, EventSink};
use config::{Config, Watcher};
use render::Renderer;
use tracing::{info, warn};

use super::bridge::{Ask, Bridge, Call};
use super::{Daemon, describe};
use crate::paths::Paths;

pub fn run(config: Config, paths: &Paths, name: &str, started: Instant) -> anyhow::Result<()> {
    // ## Bind the control socket
    //
    // First of all, so that a second daemon refuses before it has drawn anything and
    // before the compositor has been asked for a single surface.
    let server = control::Server::bind(&paths.socket)?;

    // ## Start the event loop
    //
    let mut event_loop: EventLoop<Daemon> = EventLoop::try_new()?;
    let handle = event_loop.handle();

    // ## Watch for signals
    //
    // Before any thread exists, including the renderer's: this blocks the signals here
    // and threads inherit the mask. One that did not would kill the process on receipt.
    let stop = event_loop.get_signal();
    handle
        .insert_source(Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?, move |event, _, _| {
            info!(signal = ?event.signal(), "shutting down");
            stop.stop();
        })
        .map_err(|error| anyhow::anyhow!("cannot listen for signals: {error}"))?;

    // ## Initialize the renderer
    //
    let mut renderer = Renderer::new(&config.surface)?;

    // ## Clone wayland fd & decode fd, register them into the event loop
    //
    // Duplicates of the descriptors, so the event loop owns what it polls. Both ends
    // refer to the same open file, so readiness is identical.
    let poll_fd =
        renderer.fd().try_clone_to_owned().context("cannot duplicate the Wayland connection")?;
    let decode_fd =
        renderer.decode_fd().try_clone_to_owned().context("cannot duplicate the decode signal")?;
    renderer.flush()?;
    //
    let stop = event_loop.get_signal();
    handle
        .insert_source(Generic::new(poll_fd, Interest::READ, Mode::Level), move |_, _, daemon| {
            if let Err(error) = daemon.on_display() {
                daemon.record(error);
                stop.stop();
            }
            Ok(PostAction::Continue)
        })
        .map_err(|error| anyhow::anyhow!("cannot watch the Wayland connection: {error}"))?;
    //
    // A wallpaper finishes decoding on another thread, often long after the last frame.
    // Answering the signal is all this does; the draw that follows takes the image.
    handle
        .insert_source(Generic::new(decode_fd, Interest::READ, Mode::Level), |_, _, daemon| {
            daemon.renderer.noticed_decode();
            Ok(PostAction::Continue)
        })
        .map_err(|error| anyhow::anyhow!("cannot watch for decoded wallpapers: {error}"))?;

    // ## Spawn the compositor backend thread
    //
    let running = Arc::new(AtomicBool::new(true));
    let (sender, receiver) = channel::channel();
    handle
        .insert_source(receiver, |event, _, daemon| {
            if let channel::Event::Msg(event) = event {
                daemon.on_compositor(event);
            }
        })
        .map_err(|error| anyhow::anyhow!("cannot watch the compositor: {error}"))?;
    spawn_backend(sender, Arc::clone(&running))?;

    // ## Spawn the control socket thread
    //
    let (calls, requests) = channel::channel::<Call>();
    handle
        .insert_source(requests, |event, _, daemon| {
            if let channel::Event::Msg(call) = event {
                let response = match call.ask {
                    Ask::Answer(request) => daemon.on_request(request),
                    Ask::Listen(subscriber) => daemon.on_subscribe(subscriber),
                };
                // A client that gave up in the meantime is not an error worth reporting.
                let _ = call.reply.send(response);
            }
        })
        .map_err(|error| anyhow::anyhow!("cannot watch the control socket: {error}"))?;
    server.spawn(Bridge::new(calls))?;
    info!(socket = %server.path().display(), "listening for control connections");

    // ## Read from the store (namely the cache)
    //
    // Before the daemon, which seeds the signals from it, and therefore before the first
    // output can appear and ask what it should be showing.
    let store = store::Store::open(&paths.state, &paths.cache_dir)?;
    info!(state = %store.file().display(), cache = %store.cache_dir().display(), "remembering");

    // ## Initialize the daemon (central state)
    //
    let mut daemon =
        Daemon::new(renderer, config, paths.config.clone(), name.to_owned(), started, store);
    daemon.watcher = watch_config(&handle, &paths.config);

    // ## Draw the first frame
    //
    // The compositor has already answered the surfaces created during connect, so there
    // is a first frame to draw before the loop ever blocks.
    daemon.on_display()?;
    daemon.settle()?;

    // ## Run the main event loop
    //
    let stop = event_loop.get_signal();
    // No timeout: with every animation settled there is nothing to wake up for, and the
    // process sleeps until the compositor or a client says otherwise.
    event_loop.run(None, &mut daemon, move |daemon| {
        if let Err(error) = daemon.settle() {
            daemon.record(error);
            stop.stop();
        }
    })?;

    // ## Shutdown
    //
    // The backend notices on its next read and returns. It is not joined: waiting on it
    // would add its poll interval to every shutdown for no benefit.
    running.store(false, Ordering::Relaxed);
    info!(frames = daemon.renderer.frames(), "stopped");

    // ## Return the error that caused the shutdown, if any
    //
    match daemon.failure.take() {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

/// Watches the configuration file for edits, if it can.
///
/// A directory that does not exist yet is not fatal: the daemon runs on the defaults it
/// already loaded, and `reload` over the socket still works by hand.
///
/// The watcher goes to the daemon and only its descriptor is polled, for the same reason
/// the Wayland connection is duplicated: calloop owning the value would mean an unsafe
/// borrow every time it is read.
fn watch_config(handle: &LoopHandle<'static, Daemon>, path: &Path) -> Option<Watcher> {
    let watcher = match Watcher::new(path) {
        Ok(watcher) => watcher,
        Err(error) => {
            warn!(error = describe(error), "edits to the configuration file will not be noticed");
            return None;
        }
    };

    let fd = match watcher.as_fd().try_clone_to_owned() {
        Ok(fd) => fd,
        Err(error) => {
            warn!(error = describe(error), "cannot watch the configuration file");
            return None;
        }
    };

    let source = Generic::new(fd, Interest::READ, Mode::Level);
    let registered = handle.insert_source(source, |_, _, daemon| {
        daemon.on_config_event();
        Ok(PostAction::Continue)
    });
    match registered {
        Ok(_) => Some(watcher),
        Err(error) => {
            warn!(%error, "cannot watch the configuration file");
            None
        }
    }
}

/// Runs the compositor backend on its own thread.
///
/// The backend blocks on its socket, so it cannot share the event loop. Everything it
/// observes arrives as a message, keeping every mutation of daemon state on one thread.
fn spawn_backend(
    sender: channel::Sender<CompositorEvent>,
    running: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let Some(mut backend) = compositor::backends::detect() else {
        warn!(
            supported = ?compositor::backends::AVAILABLE,
            "no compositor backend for this session, so the effects it drives stay at rest"
        );
        return Ok(());
    };

    let name = backend.name();
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let sink = Sink { sender, running };
            if let Err(error) = backend.run(&sink) {
                warn!(backend = name, %error, "compositor backend gave up");
            }
        })
        .with_context(|| format!("cannot start the {name} backend"))?;
    Ok(())
}

struct Sink {
    sender: channel::Sender<CompositorEvent>,
    running: Arc<AtomicBool>,
}

impl EventSink for Sink {
    fn emit(&self, event: CompositorEvent) {
        // A closed channel means the daemon is already on its way out.
        let _ = self.sender.send(event);
    }

    fn is_open(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}
