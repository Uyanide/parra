pub mod hyprland;
mod lines;
pub mod niri;

use std::collections::BTreeMap;
use std::fmt;

use domain::OutputId;
use serde::{Deserialize, Deserializer, de::Error as _};

use crate::{BackendError, CompositorBackend};

/// Names of every backend this build has, in the order [`detect`] tries them.
pub const AVAILABLE: &[&str] = &[niri::NAME, hyprland::NAME];

/// The compositor running here, if it is one of [`AVAILABLE`].
pub fn detect() -> Option<&'static str> {
    AVAILABLE.iter().copied().find(|name| is_running(name))
}

/// Whether one named backend could connect right now.
pub fn is_running(backend: &str) -> bool {
    match backend {
        niri::NAME => niri::is_running(),
        hyprland::NAME => hyprland::is_running(),
        _ => false,
    }
}

/// One backend's settings, globally and for the outputs that differ.
///
/// The map is inside the backend's own type rather than beside it, so two outputs cannot
/// be configured for two different compositors.
#[derive(Clone, Debug, PartialEq)]
pub struct Scoped<P> {
    global: P,
    per_output: BTreeMap<OutputId, P>,
}

impl<P> Scoped<P> {
    pub(crate) fn new(global: P) -> Self {
        Self { global, per_output: BTreeMap::new() }
    }

    /// What applies to one output, which is the global settings unless it said otherwise.
    pub fn for_output(&self, id: &OutputId) -> &P {
        self.per_output.get(id).unwrap_or(&self.global)
    }

    pub(crate) fn set_output(&mut self, id: OutputId, params: P) {
        self.per_output.insert(id, params);
    }
}

/// One backend's own settings, in the shape that backend defined.
#[derive(Clone, Debug, PartialEq)]
pub enum Settings {
    Niri(Scoped<niri::Params>),
    Hyprland(Scoped<hyprland::Params>),
}

impl Settings {
    /// Which backend these belong to, so a caller holding one need not carry the name.
    pub fn backend(&self) -> &'static str {
        match self {
            Settings::Niri(_) => niri::NAME,
            Settings::Hyprland(_) => hyprland::NAME,
        }
    }

    /// Reads one backend's settings, in whichever format the caller brings.
    ///
    /// A pure function of the name and the data: no compositor has to be running.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        backend: &str,
        de: D,
    ) -> Result<Settings, D::Error> {
        match backend {
            niri::NAME => Ok(Settings::Niri(Scoped::new(Deserialize::deserialize(de)?))),
            hyprland::NAME => Ok(Settings::Hyprland(Scoped::new(Deserialize::deserialize(de)?))),
            _ => Err(D::Error::custom(format!("no compositor backend named {backend:?}"))),
        }
    }

    /// Reads what one output says of its own, which the caller has already laid over the
    /// global section so that this reads a whole one.
    pub fn deserialize_output<'de, D: Deserializer<'de>>(
        &mut self,
        output: OutputId,
        de: D,
    ) -> Result<(), D::Error> {
        match self {
            Settings::Niri(scoped) => scoped.set_output(output, Deserialize::deserialize(de)?),
            Settings::Hyprland(scoped) => scoped.set_output(output, Deserialize::deserialize(de)?),
        }
        Ok(())
    }

    /// What a backend runs on when its section is absent entirely.
    pub fn default_for(backend: &str) -> Option<Settings> {
        match backend {
            niri::NAME => Some(Settings::Niri(Scoped::new(niri::Params::default()))),
            hyprland::NAME => Some(Settings::Hyprland(Scoped::new(hyprland::Params::default()))),
            _ => None,
        }
    }

    /// Every output configured apart from the rest, each rendered the way [`Display`]
    /// renders the global settings.
    pub fn overrides(&self) -> Vec<(&OutputId, String)> {
        match self {
            Settings::Niri(scoped) => {
                scoped.per_output.iter().map(|(id, p)| (id, p.to_string())).collect()
            }
            Settings::Hyprland(scoped) => {
                scoped.per_output.iter().map(|(id, p)| (id, p.to_string())).collect()
            }
        }
    }
}

impl fmt::Display for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Settings::Niri(scoped) => scoped.global.fmt(f),
            Settings::Hyprland(scoped) => scoped.global.fmt(f),
        }
    }
}

/// Connects to the compositor these settings are for.
pub fn connect(settings: &Settings) -> Result<Box<dyn CompositorBackend>, BackendError> {
    match settings {
        Settings::Niri(scoped) => Ok(Box::new(niri::Backend::connect(scoped.clone())?)),
        Settings::Hyprland(scoped) => Ok(Box::new(hyprland::Backend::connect(scoped.clone())?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON here, TOML in the daemon: the format is whatever the caller brings.
    fn parse(backend: &str, json: &str) -> Result<Settings, serde_json::Error> {
        Settings::deserialize(backend, &mut serde_json::Deserializer::from_str(json))
    }

    fn niri_params(settings: &Settings, output: &str) -> niri::Params {
        match settings {
            Settings::Niri(scoped) => *scoped.for_output(&OutputId::new(output)),
            other => panic!("expected niri settings, not {}", other.backend()),
        }
    }

    fn hyprland_params(settings: &Settings, output: &str) -> hyprland::Params {
        match settings {
            Settings::Hyprland(scoped) => scoped.for_output(&OutputId::new(output)).clone(),
            other => panic!("expected hyprland settings, not {}", other.backend()),
        }
    }

    #[test]
    fn an_absent_section_reads_as_the_backend_defaults() {
        for backend in AVAILABLE {
            assert_eq!(parse(backend, "{}").unwrap(), Settings::default_for(backend).unwrap());
        }
    }

    #[test]
    fn an_unknown_key_names_itself() {
        let error = parse(niri::NAME, r#"{"vertikal":"workspace"}"#).unwrap_err();
        assert!(error.to_string().contains("vertikal"), "{error}");
    }

    #[test]
    fn an_unknown_backend_is_refused_rather_than_defaulted() {
        assert!(parse("sway", "{}").is_err());
        assert_eq!(Settings::default_for("sway"), None);
    }

    #[test]
    fn parsing_needs_no_compositor_running() {
        let settings = parse(niri::NAME, r#"{"vertical":"column","horizontal":"workspace"}"#);
        assert!(settings.is_ok(), "reading settings should not need a compositor");
    }

    #[test]
    fn an_output_reads_the_global_settings_until_it_says_otherwise() {
        let mut settings = parse(niri::NAME, r#"{"horizontal":"column"}"#).unwrap();
        settings
            .deserialize_output(
                OutputId::new("DP-1"),
                &mut serde_json::Deserializer::from_str(r#"{"horizontal":"none"}"#),
            )
            .unwrap();

        assert_eq!(niri_params(&settings, "DP-1").horizontal, niri::Axis::None);
        assert_eq!(niri_params(&settings, "eDP-1").horizontal, niri::Axis::Column);
    }

    #[test]
    fn an_override_is_reported_under_its_own_output() {
        let mut settings = parse(niri::NAME, "{}").unwrap();
        settings
            .deserialize_output(
                OutputId::new("DP-1"),
                &mut serde_json::Deserializer::from_str(r#"{"horizontal":"column"}"#),
            )
            .unwrap();

        let reported = settings.overrides();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].0, &OutputId::new("DP-1"));
        assert!(reported[0].1.contains("horizontal=column"), "{}", reported[0].1);
    }

    #[test]
    fn a_bad_key_under_an_output_is_refused_like_any_other() {
        let mut settings = parse(niri::NAME, "{}").unwrap();
        let error = settings.deserialize_output(
            OutputId::new("DP-1"),
            &mut serde_json::Deserializer::from_str(r#"{"horizontal":"sideways"}"#),
        );
        assert!(error.is_err());
    }

    #[test]
    fn a_hyprland_output_reads_the_span_until_it_says_otherwise() {
        let mut settings = parse(hyprland::NAME, r#"{"span":5}"#).unwrap();
        settings
            .deserialize_output(
                OutputId::new("DP-1"),
                &mut serde_json::Deserializer::from_str(r#"{"span":["6","7","8"]}"#),
            )
            .unwrap();

        let names = ["6", "7", "8"].map(str::to_owned).to_vec();
        assert_eq!(hyprland_params(&settings, "DP-1").span, hyprland::Span::Names(names));
        assert_eq!(hyprland_params(&settings, "eDP-1").span, hyprland::Span::Count(5));
    }

    #[test]
    fn a_hyprland_override_is_reported_under_its_own_output() {
        let mut settings = parse(hyprland::NAME, "{}").unwrap();
        settings
            .deserialize_output(
                OutputId::new("DP-1"),
                &mut serde_json::Deserializer::from_str(r#"{"vertical":"workspace"}"#),
            )
            .unwrap();

        let reported = settings.overrides();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].0, &OutputId::new("DP-1"));
        assert!(reported[0].1.contains("vertical=workspace"), "{}", reported[0].1);
    }

    /// A span of nothing has nowhere to travel, and reading it as a centred axis would
    /// hide the typo rather than report it.
    #[test]
    fn an_empty_hyprland_span_is_refused() {
        assert!(parse(hyprland::NAME, r#"{"span":0}"#).is_err());
        assert!(parse(hyprland::NAME, r#"{"span":[]}"#).is_err());
    }

    /// niri's own second axis, which Hyprland has no position for.
    #[test]
    fn hyprland_has_no_column_to_follow() {
        assert!(parse(hyprland::NAME, r#"{"horizontal":"column"}"#).is_err());
    }
}
