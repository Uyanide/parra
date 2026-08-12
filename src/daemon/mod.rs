mod bridge;
mod loops;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use config::Config;
use control::{
    GpuSnapshot, Micros, OutputSnapshot, PROTOCOL_VERSION, Request, Response, StateSnapshot,
};
use domain::{Facts, MonitorState, OutputId, Signals, WallpaperRef, policy};
use render::{RenderError, RenderEvent, Renderer};
use tracing::{debug, info, trace, warn};

pub use loops::run;

/// Everything the running daemon holds.
pub struct Daemon {
    renderer: Renderer,
    config: Config,
    config_path: PathBuf,
    /// Passed to `config::load` as the fallback layer-shell namespace.
    name: String,
    states: BTreeMap<OutputId, MonitorState>,
    facts: Facts,
    signals: Signals,
    /// Per output, so two monitors at different refresh rates each advance by their own
    /// elapsed time rather than a shared one.
    clocks: BTreeMap<OutputId, Instant>,
    /// Watches the configuration file. `None` when its directory could not be watched, in
    /// which case reloading happens only when something asks for it.
    watcher: Option<config::Watcher>,
    /// When the process began, and how long it took to put something on a screen. The
    /// second is measured once and kept: it is a fact about this start.
    started: Instant,
    startup_us: Option<Micros>,
    /// Set when the facts or signals changed and the targets have not been re-derived
    /// yet. Batches a burst of compositor events into one resolve.
    stale: bool,
    /// First error out of the renderer. The event loop cannot carry it, so it waits here
    /// until the loop has stopped.
    failure: Option<RenderError>,
}

impl Daemon {
    fn new(
        renderer: Renderer,
        config: Config,
        config_path: PathBuf,
        name: String,
        started: Instant,
    ) -> Self {
        Self {
            renderer,
            config,
            config_path,
            name,
            states: BTreeMap::new(),
            facts: Facts::default(),
            signals: Signals::default(),
            clocks: BTreeMap::new(),
            watcher: None,
            started,
            startup_us: None,
            stale: false,
            failure: None,
        }
    }

    /// Something happened in the configuration file's directory. Most of those are an
    /// editor's temporary files, so the watcher is asked whether ours was among them.
    fn on_config_event(&mut self) {
        if self.watcher.as_mut().is_some_and(config::Watcher::changed) {
            // Whatever went wrong is already in the log, and nobody asked for this one.
            let _ = self.reload();
        }
    }

    /// Folds one normalized fact into the accumulated view of the world.
    ///
    /// A single compositor action produces a burst of facts, so the resolve waits for
    /// the end of the loop iteration and happens once.
    fn on_compositor(&mut self, event: compositor::CompositorEvent) {
        self.stale |= event.apply_to(&mut self.facts);
    }

    /// Brings the screen up to date, then drains anything that arrived while doing so.
    ///
    /// Presenting reads the Wayland connection itself, so a frame callback can land in
    /// the queue with nothing left on the descriptor for the event loop to notice.
    /// Looping until the queue is genuinely empty is the only safe point to sleep at;
    /// without it, about half of all runs stranded an animation part-way through.
    fn settle(&mut self) -> Result<(), RenderError> {
        loop {
            if std::mem::take(&mut self.stale) {
                self.resolve();
            }
            self.renderer.draw(&self.states)?;
            self.note_first_frame();

            let queued = self.renderer.dispatch_queued()?;
            if queued.is_empty() {
                return Ok(());
            }
            self.consume(queued);
        }
    }

    /// Records how long the cold start took, the first time anything is on screen.
    ///
    /// Measured to the first frame rather than to the surfaces being created, so
    /// decoding the first wallpaper counts towards it.
    fn note_first_frame(&mut self) {
        if self.startup_us.is_some() || self.renderer.frames() == 0 {
            return;
        }
        let elapsed = self.started.elapsed();
        info!(elapsed = ?elapsed, "first frame on screen");
        self.startup_us = Some(elapsed.as_micros().try_into().unwrap_or(Micros::MAX));
    }

    /// Reads what the display server has to say and advances whatever it implies.
    fn on_display(&mut self) -> Result<(), RenderError> {
        let events = self.renderer.dispatch()?;
        self.consume(events);
        Ok(())
    }

    fn consume(&mut self, events: Vec<RenderEvent>) {
        for event in events {
            match event {
                RenderEvent::OutputReady { id, logical, scale } => match self.states.get_mut(&id) {
                    Some(state) => {
                        state.logical = logical;
                        state.scale = scale;
                    }
                    None => self.add_output(id, logical, scale),
                },
                RenderEvent::OutputGone { id } => {
                    debug!(output = %id, "dropping output state");
                    self.states.remove(&id);
                    self.clocks.remove(&id);
                }
                RenderEvent::FrameDue { id } => self.tick(&id),
            }
        }
    }

    /// A monitor appearing snaps to its resolved values rather than animating to them:
    /// coming into existence should not look like a transition.
    fn add_output(&mut self, id: OutputId, logical: domain::LogicalSize, scale: domain::Scale) {
        info!(output = %id, width = logical.w, height = logical.h, %scale, "output ready");
        let mut state = MonitorState::new(id.clone(), self.config.for_output(&id).clone());
        state.logical = logical;
        state.scale = scale;
        if let Some(wallpaper) = self.signals.wallpaper(&id) {
            state.set_wallpaper(Some(wallpaper.clone()));
        }
        state.snap(&policy::resolve(&id, &self.facts, &self.signals, &state.params));
        self.clocks.insert(id.clone(), Instant::now());
        self.states.insert(id, state);
    }

    /// Advances one output by the time actually elapsed since it was last drawn.
    fn tick(&mut self, id: &OutputId) {
        let now = Instant::now();
        let previous = self.clocks.insert(id.clone(), now).unwrap_or(now);
        trace!(output = %id, dt = ?now.duration_since(previous), "frame due");
        if let Some(state) = self.states.get_mut(id) {
            state.tick(now.duration_since(previous).as_secs_f32());
        }
    }

    /// Re-derives every output's targets and eases toward them. Called when the facts,
    /// the signals or the configuration change, never when only geometry did.
    fn resolve(&mut self) {
        // The animations start now, so the clocks do too. A clock left at the last frame
        // of the previous one would spend the whole idle period on the first tick.
        let now = Instant::now();
        for (id, state) in &mut self.states {
            let targets = policy::resolve(id, &self.facts, &self.signals, &state.params);
            let workspace = self.facts.output(id).workspace;
            debug!(
                output = %id,
                workspace = format!("{}/{}", workspace.idx, workspace.count),
                scroll = targets.scroll_v,
                blur = targets.blur,
                zoom = targets.zoom,
                "resolved"
            );
            state.apply(&targets);
            self.clocks.insert(id.clone(), now);
        }
    }

    fn record(&mut self, error: RenderError) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
    }

    /// Answers one control request, on the event loop's thread like everything else that
    /// touches state. The settle that follows is what turns the result into frames.
    fn on_request(&mut self, request: Request) -> Response {
        match request {
            Request::Ping => Response::Pong { version: PROTOCOL_VERSION },
            Request::GetState => Response::State(StateSnapshot {
                version: PROTOCOL_VERSION,
                namespace: self.config.surface.namespace.clone(),
                frames: self.renderer.frames(),
                texture_bytes: self.renderer.texture_bytes(),
                startup_us: self.startup_us,
                outputs: self.states.values().map(|state| self.snapshot(state)).collect(),
            }),
            Request::GetOutput { output } => match self.states.get(&output) {
                Some(state) => Response::Output(self.snapshot(state)),
                None => unknown_output(&output),
            },
            Request::SetWallpaper { output, path } => self.on_set_wallpaper(output, path),
            Request::SetBlur { output, on } => {
                if let Some(id) = &output
                    && !self.states.contains_key(id)
                {
                    return unknown_output(id);
                }
                debug!(output = ?output, on, "external blur signal");
                self.signals.set_blur(output, on);
                self.stale = true;
                Response::Done
            }
            Request::ReloadConfig => match self.reload() {
                Ok(()) => Response::Done,
                Err(message) => Response::Error { message },
            },
        }
    }

    /// Puts one output on the wire, measurement included.
    ///
    /// `FrameCost` and `GpuSnapshot` are the same two numbers in two crates that cannot
    /// see each other, so joining them happens here.
    fn snapshot(&self, state: &MonitorState) -> OutputSnapshot {
        let cost = self.renderer.frame_cost(&state.id);
        let gpu = GpuSnapshot { last_us: cost.last_us(), peak_us: cost.peak_us() };
        OutputSnapshot::new(state, &self.facts, gpu)
    }

    fn on_set_wallpaper(&mut self, output: Option<OutputId>, path: PathBuf) -> Response {
        if let Some(id) = &output
            && !self.states.contains_key(id)
        {
            return unknown_output(id);
        }
        // Checked while the client is still on the line. Decoding happens later and off
        // this thread, where a missing path could only be reported to the log.
        if !path.is_file() {
            return Response::Error { message: format!("{} is not a file", path.display()) };
        }

        info!(output = ?output, path = %path.display(), "setting wallpaper");
        let wallpaper = WallpaperRef::new(path);
        self.signals.set_wallpaper(output.clone(), wallpaper.clone());
        for (id, state) in &mut self.states {
            if output.as_ref().is_none_or(|wanted| wanted == id) {
                state.set_wallpaper(Some(wallpaper.clone()));
            }
        }
        Response::Done
    }

    /// Re-reads the configuration file and adopts it.
    ///
    /// A file that no longer parses leaves the running configuration in place: an edit in
    /// progress should not take the wallpaper down with it. The message is what the client
    /// is told, if there is one.
    fn reload(&mut self) -> Result<(), String> {
        let loaded = config::load(&self.config_path, &self.name).map_err(|error| {
            let message = describe(error);
            warn!(error = %message, "keeping the configuration already loaded");
            message
        })?;

        let mut next = loaded.config;
        if next.surface != self.config.surface {
            // A layer surface is given its namespace and layer when it is created, so
            // keeping the running values is what keeps `state` describing the screen.
            warn!(
                "the namespace and layer are fixed when a layer surface is created, \
                 so they take effect on the next start"
            );
            next.surface = self.config.surface.clone();
        }

        if next == self.config {
            debug!(path = %self.config_path.display(), "reloaded, nothing changed");
            return Ok(());
        }

        info!(path = %self.config_path.display(), "configuration reloaded");
        self.config = next;

        let mut reclaimed = false;
        for (id, state) in &mut self.states {
            reclaimed |= state.apply_params(self.config.for_output(id).clone());
        }
        if reclaimed {
            self.signals.forget_wallpapers();
        }

        self.stale = true;
        Ok(())
    }
}

fn unknown_output(id: &OutputId) -> Response {
    Response::Error { message: format!("no output named {id}") }
}

/// One line carrying the whole error chain. Only the outermost message would reach a log
/// line otherwise, and for anything file-related the cause is the half worth reading.
fn describe(error: impl Into<anyhow::Error>) -> String {
    format!("{:#}", error.into())
}
