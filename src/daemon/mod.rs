mod bridge;
mod loops;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, RegistrationToken};
use config::Config;
use control::{
    Event, GpuSnapshot, Micros, OutputSnapshot, PROTOCOL_VERSION, Property, Request, Response,
    StateSnapshot, Subscriber, Subscribers,
};
use domain::{Driven, MonitorState, Moves, OutputId, PixelSize, Signals, WallpaperRef, policy};
use render::{Cache, RenderError, RenderEvent, Renderer};
use store::Store;
use tracing::{debug, info, trace, warn};

pub use loops::run;

/// How long the configuration file has to stay still before it is read. A save arrives as
/// a burst: the file is moved away, created empty, then filled in.
const SETTLED: Duration = Duration::from_millis(50);

/// Everything the daemon needs that was settled before it started.
pub struct Startup {
    pub config: Config,
    /// `None` when no backend was detected and none was named, so no file applied.
    pub config_path: Option<PathBuf>,
    pub backend: Option<compositor::backends::Settings>,
}

/// Everything the running daemon holds.
pub struct Daemon {
    renderer: Renderer,
    config: Config,
    config_path: Option<PathBuf>,
    /// Fallback layer-shell namespace.
    name: String,
    states: BTreeMap<OutputId, MonitorState>,
    driven: Driven,
    signals: Signals,
    store: Store,
    /// Everyone listening for what the daemon decides. A field rather than something
    /// reached through `self`, so emitting borrows only this while the states are held.
    subscribers: Subscribers,
    /// Per output, so two monitors at different refresh rates each advance by their own
    /// elapsed time rather than a shared one.
    clocks: BTreeMap<OutputId, Instant>,
    /// Watches the configuration file. `None` when its directory could not be watched, in
    /// which case reloading happens only when something asks for it.
    watcher: Option<config::Watcher>,
    /// When the file was last touched, plus [`SETTLED`]. Every event pushes it back, so
    /// one save is one reload however many events it takes.
    reload_at: Option<Instant>,
    /// The timer waiting on it, so a burst arms one rather than one each.
    reload_timer: Option<RegistrationToken>,
    /// When the process began, and how long it took to put something on a screen.
    started: Instant,
    startup_us: Option<Micros>,
    /// Set when the driven channels or signals changed and the targets have not been
    /// re-derived yet. Batches a burst of compositor events into one resolve.
    stale: bool,
    /// First error out of the renderer. The event loop cannot carry it, so it waits here
    /// until the loop has stopped.
    failure: Option<RenderError>,
}

impl Daemon {
    fn new(
        renderer: Renderer,
        config: Config,
        config_path: Option<PathBuf>,
        name: String,
        started: Instant,
        store: Store,
    ) -> Self {
        let mut daemon = Self {
            renderer,
            config,
            config_path,
            name,
            states: BTreeMap::new(),
            driven: Driven::default(),
            signals: Signals::default(),
            store,
            subscribers: Subscribers::default(),
            clocks: BTreeMap::new(),
            watcher: None,
            reload_at: None,
            reload_timer: None,
            started,
            startup_us: None,
            stale: false,
            failure: None,
        };
        daemon.restore(None);
        daemon
    }

    /// Puts what is written down back into the signals, dropping whatever was asked for
    /// since. `None` addresses every slot, the same rule `Signals::set_wallpaper` follows.
    ///
    /// The record is read from the store's own copy rather than from the file. The two
    /// are the same thing, since a set that was not to be remembered never reached
    /// either; why that is the answer is in `docs/architecture.md`.
    fn restore(&mut self, output: Option<&OutputId>) {
        let recorded: Vec<(Option<OutputId>, WallpaperRef)> = self
            .store
            .entries()
            .filter(|(slot, _)| output.is_none() || *slot == output)
            .map(|(slot, entry)| (slot.cloned(), entry.wallpaper()))
            .collect();

        // Emptied first, so a slot the record no longer names is dropped rather than left
        // holding what a transient set put there. Startup finds it already empty.
        self.signals.set_wallpaper(output.cloned(), None);

        // The broadcast entry comes first, which is what makes this safe: applying it
        // clears the per-output slots, so the overrides have to follow it.
        for (slot, wallpaper) in recorded {
            info!(
                output = ?slot,
                path = %wallpaper.path().display(),
                "restoring a recorded wallpaper"
            );
            self.announce(&wallpaper);
            self.signals.set_wallpaper(slot, Some(wallpaper));
        }
    }

    /// Tells the renderer where this wallpaper's resized copy belongs, so a decode either
    /// reads one or leaves one behind.
    ///
    /// Every path that puts a wallpaper on a slot calls this, which is what keeps the
    /// renderer's view of the cache from drifting away from the store's.
    fn announce(&mut self, wallpaper: &WallpaperRef) {
        let cache = self.store.cached(wallpaper).map(|(file, asked)| Cache { file, asked });
        self.renderer.set_cache(wallpaper, cache);
    }

    /// Pushes what each output should now be showing.
    ///
    /// The single answer to "what should this output show", so a set, a clear, a reload
    /// and a decode that failed cannot drift apart. Clearing in particular is not "show
    /// nothing": an output that loses its own wallpaper falls back to the broadcast one
    /// and then to the config file, which is exactly what `wallpaper_for` walks.
    ///
    /// `unusable` names one the loader has just given up on, so an output whose only
    /// remaining answer is that same file shows nothing rather than asking again.
    fn resolve_wallpapers(&mut self, unusable: Option<&WallpaperRef>) {
        let resolved: Vec<(OutputId, Option<WallpaperRef>)> = self
            .states
            .iter()
            .map(|(id, state)| {
                let next = policy::wallpaper_for(&self.signals, id, &state.params)
                    .filter(|next| Some(*next) != unusable)
                    .cloned();
                (id.clone(), next)
            })
            .collect();

        // A transition starts now, so the clock carrying it does too. One left at the
        // last frame drawn would spend the whole idle period on the first step.
        let now = Instant::now();
        for (id, wallpaper) in resolved {
            if let Some(wallpaper) = &wallpaper {
                self.announce(wallpaper);
            }
            let swap = self.states.get_mut(&id).and_then(|state| state.set_wallpaper(wallpaper));
            let Some(swap) = swap else { continue };
            self.clocks.insert(id.clone(), now);
            self.subscribers.emit(&Event::wallpaper_changed(&id, &swap));
        }
    }

    /// Something happened in the configuration file's directory. Most of those are an
    /// editor's temporary files, so the watcher is asked whether ours was among them.
    fn on_config_event(&mut self, handle: &LoopHandle<'static, Daemon>) {
        if !self.watcher.as_mut().is_some_and(config::Watcher::changed) {
            return;
        }
        self.reload_at = Some(Instant::now() + SETTLED);
        if self.reload_timer.is_some() {
            return;
        }
        match handle.insert_source(Timer::from_duration(SETTLED), |_, _, daemon| daemon.settled()) {
            Ok(token) => self.reload_timer = Some(token),
            Err(error) => {
                warn!(%error, "cannot wait for the edit to finish, reading the file now");
                self.reload_at = None;
                let _ = self.reload();
            }
        }
    }

    /// Reloads once the file has been quiet for [`SETTLED`], and waits again otherwise.
    fn settled(&mut self) -> TimeoutAction {
        match self.reload_at {
            Some(deadline) if Instant::now() < deadline => TimeoutAction::ToInstant(deadline),
            _ => {
                self.reload_at = None;
                self.reload_timer = None;
                // Whatever went wrong is already in the log, and nobody asked for this one.
                let _ = self.reload();
                TimeoutAction::Drop
            }
        }
    }

    /// Folds one driven channel into the accumulated view of the world.
    ///
    /// A single compositor action produces a burst of these, so the resolve waits for
    /// the end of the loop iteration and happens once.
    fn on_compositor(&mut self, drive: compositor::Drive) {
        self.stale |= drive.apply_to(&mut self.driven);
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
            let drawn = self.renderer.draw(&self.states)?;
            self.note_first_frame();

            let queued = self.renderer.dispatch_queued()?;
            if drawn.is_empty() && queued.is_empty() {
                return Ok(());
            }
            self.consume(drawn);
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
                    self.subscribers.emit(&Event::OutputGone { output: id });
                }
                RenderEvent::FrameDue { id } => self.tick(&id),
                RenderEvent::WallpaperStored { wallpaper, asked } => {
                    self.on_wallpaper_stored(&wallpaper, asked);
                }
                RenderEvent::WallpaperFailed { wallpaper } => {
                    self.on_wallpaper_failed(&wallpaper);
                }
            }
        }
    }

    /// A monitor appearing snaps to its resolved values rather than animating to them:
    /// coming into existence should not look like a transition. Its wallpaper is the one
    /// thing that does animate, since arriving is exactly what it has to show.
    fn add_output(&mut self, id: OutputId, logical: domain::LogicalSize, scale: domain::Scale) {
        info!(output = %id, width = logical.w, height = logical.h, %scale, "output ready");
        let params = self.config.for_output(&id).clone();
        let wallpaper = policy::wallpaper_for(&self.signals, &id, &params).cloned();
        if let Some(wallpaper) = &wallpaper {
            self.announce(wallpaper);
        }

        let mut state = MonitorState::new(id.clone(), params);
        state.logical = logical;
        state.scale = scale;
        // Beside the geometry rather than left to the next resolve: arriving does not set
        // `stale`, so an output whose compositor says nothing more would draw uncapped.
        state.stride = self.driven.output(&id).stride();
        state.snap(&policy::resolve(&id, &self.driven, &self.signals, &state.params));
        let swap = state.set_wallpaper(wallpaper);
        self.clocks.insert(id.clone(), Instant::now());
        // Snapped, so no animation event will ever carry these values: the arrival does.
        self.subscribers.emit(&Event::output_ready(&state));
        // After it, and separately, because the wallpaper is the one value that did not
        // snap. Without this a listener is never told the arrival is under way.
        if let Some(swap) = swap {
            self.subscribers.emit(&Event::wallpaper_changed(&id, &swap));
        }
        self.states.insert(id, state);
    }

    /// Records that a resized copy now exists, so the next start reads it instead of
    /// decoding the original again.
    fn on_wallpaper_stored(&mut self, wallpaper: &WallpaperRef, asked: PixelSize) {
        if !self.store.stored(wallpaper, asked) {
            return;
        }
        self.announce(wallpaper);
        if let Err(error) = self.store.save() {
            warn!(error = %describe(error), "cannot record the cached wallpaper");
        }
    }

    /// Falls back for every output that was waiting on a wallpaper which will not load.
    ///
    /// The signal is dropped, or the next reload would resolve the same unusable path
    /// straight back onto the slot and find the loader has already given up on it. What
    /// is written down is deliberately left alone: this start shows the fallback, and the
    /// next one tries again, which is the point of remembering a choice rather than a
    /// file that happened to be readable.
    fn on_wallpaper_failed(&mut self, wallpaper: &WallpaperRef) {
        warn!(
            path = %wallpaper.path().display(),
            "falling back for now, and trying this again on the next start"
        );
        // Ahead of the fallbacks it causes, so a listener reads the reason before the
        // consequences. Nothing else reports this: the client that asked was told `done`.
        self.subscribers.emit(&Event::WallpaperFailed { path: wallpaper.path().to_path_buf() });
        self.signals.forget_wallpaper(wallpaper);
        // The configured fallback may be the very thing that just failed, which is why
        // this is not simply a re-resolve: showing nothing is the honest answer there,
        // and is also what stops it repeating.
        self.resolve_wallpapers(Some(wallpaper));
    }

    /// Advances one output by the time since it was last drawn.
    fn tick(&mut self, id: &OutputId) {
        let now = Instant::now();
        let previous = self.clocks.insert(id.clone(), now).unwrap_or(now);
        let dt = step(previous, now);
        trace!(output = %id, dt, "frame due");
        if let Some(state) = self.states.get_mut(id) {
            state.tick(dt);
        }
    }

    /// Re-derives every output's targets and eases toward them. Called when the driven
    /// channels, the signals or the configuration change, never when only geometry did.
    fn resolve(&mut self) {
        // The animations start now, so the clocks do too. A clock left at the last frame
        // of the previous one would spend the whole idle period on the first tick.
        let now = Instant::now();
        for (id, state) in &mut self.states {
            let targets = policy::resolve(id, &self.driven, &self.signals, &state.params);
            let channels = self.driven.output(id);
            // Read at sample time rather than animated toward, so it is assigned here
            // rather than reaching the state through `Targets`. A change to it moves the
            // sampled rect with the position standing still, which is what opening a
            // workspace does, so it is one of the two things owing a frame below.
            let stride = channels.stride();
            let restrided = state.stride != stride;
            state.stride = stride;
            debug!(
                output = %id,
                driven = %format!("{:.3},{:.3}", channels.x.at, channels.y.at),
                stride = %format!("{:.3},{:.3}", channels.x.stride, channels.y.stride),
                scroll = %format!("{:.3},{:.3}", targets.scroll_h, targets.scroll_v),
                blur = targets.blur,
                zoom = targets.zoom,
                "resolved"
            );
            let moves = state.apply(&targets);
            self.clocks.insert(id.clone(), now);
            // A move that takes time is drawn because it is still running. One of no
            // duration has settled by the next pass, so this is the only report of it.
            if restrided || moves != Moves::default() {
                self.renderer.invalidate(id);
            }
            announce_moves(&mut self.subscribers, id, moves);
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
            Request::SetWallpaper { output, path, save } => {
                self.on_set_wallpaper(output, path, save)
            }
            Request::RestoreWallpaper { output } => {
                if let Some(id) = &output
                    && !self.states.contains_key(id)
                {
                    return unknown_output(id);
                }
                info!(output = ?output, "restoring the recorded wallpapers");
                self.restore(output.as_ref());
                // Nothing animated moved, so no resolve is owed. A wallpaper the renderer
                // still holds a texture for is drawn anyway: `Renderer::draw` compares what
                // it presented against the slot and marks the surface dirty itself.
                self.resolve_wallpapers(None);
                Response::Done
            }
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
            // The socket answers this one itself, since the queue it needs belongs to the
            // connection rather than to the loop.
            Request::Subscribe => Response::Error {
                message: "subscribe is answered on the connection it arrives on".to_owned(),
            },
        }
    }

    /// Takes on a listener, first describing every output that already exists.
    ///
    /// Nothing can be emitted between the description and the registration: both happen
    /// here, on the one thread that emits at all.
    fn on_subscribe(&mut self, subscriber: Subscriber) -> Response {
        for state in self.states.values() {
            if !subscriber.send(&Event::output_ready(state)) {
                return Response::Error { message: "cannot start the event stream".to_owned() };
            }
        }
        debug!(outputs = self.states.len(), "a listener subscribed");
        self.subscribers.add(subscriber);
        Response::Done
    }

    /// Puts one output on the wire, measurement included.
    ///
    /// `FrameCost` and `GpuSnapshot` are the same two numbers in two crates that cannot
    /// see each other, so joining them happens here.
    fn snapshot(&self, state: &MonitorState) -> OutputSnapshot {
        let cost = self.renderer.frame_cost(&state.id);
        let gpu = GpuSnapshot { last_us: cost.last_us(), peak_us: cost.peak_us() };
        OutputSnapshot::new(state, &self.driven, gpu)
    }

    /// Every set is a new wallpaper, even one naming the path already on screen.
    ///
    /// That is the epoch's doing rather than a comparison here: it makes the identity
    /// different, so the slot, the texture cache and the loader all reload without being
    /// told to. An image edited in place therefore takes effect too.
    ///
    /// No path empties the slot instead. What that output then shows is resolved rather
    /// than blanked, so removing an override reveals the broadcast wallpaper underneath.
    fn on_set_wallpaper(
        &mut self,
        output: Option<OutputId>,
        path: Option<PathBuf>,
        save: bool,
    ) -> Response {
        if let Some(id) = &output
            && !self.states.contains_key(id)
        {
            return unknown_output(id);
        }
        // Checked while the client is still on the line. Decoding happens later and off
        // this thread, where a missing path could only be reported to the log.
        if let Some(path) = &path
            && !path.is_file()
        {
            return Response::Error { message: format!("{} is not a file", path.display()) };
        }

        info!(output = ?output, path = ?path, save, "setting wallpaper");
        let wallpaper = match path {
            Some(path) if save => Some(self.store.set(output.as_ref(), path)),
            Some(path) => Some(self.store.transient(path)),
            None => {
                if save {
                    self.store.clear(output.as_ref());
                }
                None
            }
        };

        // Written before the decode, referring to a copy that does not exist yet: a crash
        // in between leaves an entry with no size recorded, which is regenerated from the
        // original rather than trusted. The same call sweeps whatever it stopped naming.
        if save && let Err(error) = self.store.save() {
            warn!(error = %describe(error), "cannot remember this wallpaper");
        }

        self.signals.set_wallpaper(output, wallpaper);
        self.resolve_wallpapers(None);
        Response::Done
    }

    /// Re-reads the configuration file and adopts it.
    ///
    /// A file that no longer parses leaves the running configuration in place: an edit in
    /// progress should not take the wallpaper down with it. The message is what the client
    /// is told, if there is one.
    fn reload(&mut self) -> Result<(), String> {
        let Some(path) = self.config_path.clone() else {
            let message = "there is no configuration file to reload".to_owned();
            warn!(error = %message, "nothing to do");
            return Err(message);
        };
        let loaded = config::load(&path, &self.name).map_err(|error| {
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
        let compositor_edited = next.compositor_table() != self.config.compositor_table()
            || !next.compositor_overrides().eq(self.config.compositor_overrides());
        if compositor_edited {
            // Not kept like the surface is: nothing reads it back, and the running
            // backend goes on driving the channels the way it was started with.
            warn!(
                "compositor settings are read when the backend connects, \
                 so they take effect on the next start"
            );
        }

        if next == self.config {
            debug!(path = %path.display(), "reloaded, nothing changed");
            return Ok(());
        }

        info!(path = %path.display(), "configuration reloaded");
        self.config = next;
        // Ahead of the params and the wallpapers, so a listener reads this before whatever
        // it set moving. The parameters themselves are read back with `get-state`.
        self.subscribers.emit(&Event::ConfigReloaded);

        let ids: Vec<OutputId> = self.states.keys().cloned().collect();
        for id in ids {
            let params = self.config.for_output(&id).clone();
            if let Some(state) = self.states.get_mut(&id) {
                state.apply_params(params);
            }
            // Several parameters are read where the frame is built rather than eased
            // toward -- `scroll.<axis>.max-shift` and the whole of the blur's look -- so
            // an edit touching only those starts no animation to be drawn by.
            self.renderer.invalidate(&id);
        }
        // After the params, since that is where the edited fallback arrives. It reaches
        // only the outputs actually showing one: what was set over the socket owns its
        // slot, and the file no longer competes for it.
        self.resolve_wallpapers(None);

        self.stale = true;
        Ok(())
    }
}

fn unknown_output(id: &OutputId) -> Response {
    Response::Error { message: format!("no output named {id}") }
}

/// Puts whichever animations just started on the wire, one line each.
///
/// Free rather than a method because the states are borrowed for the length of a resolve,
/// so the listeners have to be reached without going through the whole daemon.
fn announce_moves(subscribers: &mut Subscribers, output: &OutputId, moves: Moves) {
    let by_property = [
        (Property::ScrollVertical, moves.scroll_v),
        (Property::ScrollHorizontal, moves.scroll_h),
        (Property::Blur, moves.blur),
        (Property::Zoom, moves.zoom),
    ];
    for (property, started) in by_property {
        if let Some(started) = started {
            subscribers.emit(&Event::animation(output, property, started));
        }
    }
}

/// Longest an animation advances in one step. Comfortably above any real refresh
/// interval, so a frame that arrives on time is never shortened.
const MAX_STEP: Duration = Duration::from_millis(100);

/// Time between two frames of one output, bounded by [`MAX_STEP`].
///
/// An output submits nothing while it is idle or waiting on a decode, so its clock can be
/// arbitrarily old, and an unbounded step would spend a whole animation on one frame.
fn step(previous: Instant, now: Instant) -> f32 {
    now.duration_since(previous).min(MAX_STEP).as_secs_f32()
}

/// One line carrying the whole error chain. Only the outermost message would reach a log
/// line otherwise, and for anything file-related the cause is the half worth reading.
fn describe(error: impl Into<anyhow::Error>) -> String {
    format!("{:#}", error.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_arriving_on_time_is_not_shortened() {
        let previous = Instant::now();
        let dt = step(previous, previous + Duration::from_millis(16));
        assert!((dt - 0.016).abs() < 1e-6);
    }

    #[test]
    fn a_stale_clock_cannot_spend_a_whole_animation_on_one_frame() {
        let previous = Instant::now();
        assert_eq!(step(previous, previous + Duration::from_secs(60)), MAX_STEP.as_secs_f32());
    }
}
