use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use domain::anim::seconds_from_millis;
use domain::params::MIN_CROP_RATIO;
use domain::{
    AxisParams, BlurParams, Easing, OutputId, OutputParams, OverviewParams, ScrollParams,
    SurfaceParams, TransitionParams, Tween, WallpaperRef,
};

use crate::schema::{
    AxisSection, BlurSection, ConfigFile, OutputSection, OverviewSection, ScrollSection,
    TransitionSection, WallpaperSection,
};

/// Enough passes to blur a 4K image beyond recognition; past this the bake cost stops
/// buying anything visible.
pub const MAX_BLUR_RADIUS: u32 = 512;
/// Below a sixteenth the blur texture starts showing its own sampling grid.
pub const MAX_BLUR_DOWNSCALE: u32 = 16;
/// A minute is already far outside any plausible transition.
pub const MAX_DURATION_MS: u32 = 60_000;

/// Where a value came from, so an error can name the exact TOML key.
type Result<T> = std::result::Result<T, Invalid>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub key: String,
    pub message: String,
}

impl Invalid {
    fn new(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self { key: key.into(), message: message.into() }
    }
}

/// Facts the config file cannot supply for itself.
///
/// `default_namespace` is injected so the program's name keeps its single definition
/// point, in the root `Cargo.toml`.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    pub default_namespace: &'a str,
    /// Directory that relative paths resolve against, normally the config file's own.
    pub base_dir: &'a Path,
    pub home: Option<&'a Path>,
}

/// The config file in semantic form: durations in seconds, colours parsed, paths
/// absolute, per-output tables already merged over the global ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub surface: SurfaceParams,
    pub global: OutputParams,
    per_output: BTreeMap<OutputId, OutputParams>,
}

impl Config {
    /// Falls back to the global parameters, so an output with no table of its own stays
    /// an ordinary case everywhere else.
    pub fn for_output(&self, id: &OutputId) -> &OutputParams {
        self.per_output.get(id).unwrap_or(&self.global)
    }

    pub fn configured_outputs(&self) -> impl Iterator<Item = &OutputId> {
        self.per_output.keys()
    }
}

/// Every section that can appear both globally and under one output.
struct Sections<'a> {
    wallpaper: &'a WallpaperSection,
    scroll: &'a ScrollSection,
    blur: &'a BlurSection,
    overview: &'a OverviewSection,
    transition: &'a TransitionSection,
}

impl<'a> Sections<'a> {
    fn global(file: &'a ConfigFile) -> Self {
        Self {
            wallpaper: &file.wallpaper,
            scroll: &file.scroll,
            blur: &file.blur,
            overview: &file.overview,
            transition: &file.transition,
        }
    }

    fn output(section: &'a OutputSection) -> Self {
        Self {
            wallpaper: &section.wallpaper,
            scroll: &section.scroll,
            blur: &section.blur,
            overview: &section.overview,
            transition: &section.transition,
        }
    }
}

pub fn resolve(file: &ConfigFile, ctx: &Context<'_>) -> Result<Config> {
    let mut global = OutputParams::default();
    apply(&mut global, Sections::global(file), "", ctx)?;

    let mut per_output = BTreeMap::new();
    for (name, section) in &file.output {
        if name.is_empty() {
            return Err(Invalid::new("output", "an output table needs a connector name"));
        }
        let prefix = format!("output.{name:?}.");
        let mut params = global.clone();
        apply(&mut params, Sections::output(section), &prefix, ctx)?;
        per_output.insert(OutputId::new(name.clone()), params);
    }

    let namespace = match &file.general.namespace {
        Some(value) if value.trim().is_empty() => {
            return Err(Invalid::new("general.namespace", "must not be empty"));
        }
        Some(value) => value.clone(),
        None => ctx.default_namespace.to_owned(),
    };
    let surface = SurfaceParams { namespace, layer: file.general.layer.unwrap_or_default() };

    Ok(Config { surface, global, per_output })
}

fn apply(
    params: &mut OutputParams,
    sections: Sections<'_>,
    prefix: &str,
    ctx: &Context<'_>,
) -> Result<()> {
    apply_wallpaper(params, sections.wallpaper, prefix, ctx)?;
    apply_scroll(&mut params.scroll, sections.scroll, prefix)?;
    apply_blur(&mut params.blur, sections.blur, prefix)?;
    apply_overview(&mut params.overview, sections.overview, prefix)?;
    apply_transition(&mut params.transition, sections.transition, prefix)
}

fn apply_wallpaper(
    params: &mut OutputParams,
    section: &WallpaperSection,
    prefix: &str,
    ctx: &Context<'_>,
) -> Result<()> {
    if let Some(raw) = &section.fallback {
        let key = format!("{prefix}wallpaper.fallback");
        params.fallback = Some(WallpaperRef::new(resolve_path(raw, ctx, &key)?));
    }
    Ok(())
}

fn apply_scroll(params: &mut ScrollParams, section: &ScrollSection, prefix: &str) -> Result<()> {
    apply_axis(&mut params.vertical, &section.vertical, &format!("{prefix}scroll.vertical"))?;
    apply_axis(&mut params.horizontal, &section.horizontal, &format!("{prefix}scroll.horizontal"))
}

fn apply_axis(params: &mut AxisParams, section: &AxisSection, path: &str) -> Result<()> {
    overwrite(&mut params.enabled, section.enabled);
    set_ratio(&mut params.travel, section.travel, &format!("{path}.travel"))?;
    apply_tween(&mut params.tween, section.duration_ms, section.easing, path)
}

fn apply_blur(params: &mut BlurParams, section: &BlurSection, prefix: &str) -> Result<()> {
    set_bounded(
        &mut params.radius,
        section.radius,
        0,
        MAX_BLUR_RADIUS,
        &format!("{prefix}blur.radius"),
    )?;
    set_bounded(
        &mut params.downscale,
        section.downscale,
        1,
        MAX_BLUR_DOWNSCALE,
        &format!("{prefix}blur.downscale"),
    )?;
    overwrite(&mut params.tint, section.tint);
    set_ratio(
        &mut params.tint_opacity,
        section.tint_opacity,
        &format!("{prefix}blur.tint-opacity"),
    )?;
    apply_tween(&mut params.tween, section.duration_ms, section.easing, &format!("{prefix}blur"))
}

fn apply_overview(
    params: &mut OverviewParams,
    section: &OverviewSection,
    prefix: &str,
) -> Result<()> {
    if let Some(value) = section.crop_ratio {
        let key = format!("{prefix}overview.crop-ratio");
        if !value.is_finite() || !(MIN_CROP_RATIO..=1.0).contains(&value) {
            return Err(Invalid::new(key, format!("expected a number in {MIN_CROP_RATIO}..=1")));
        }
        params.crop_ratio = value;
    }
    apply_tween(
        &mut params.tween,
        section.duration_ms,
        section.easing,
        &format!("{prefix}overview"),
    )
}

fn apply_transition(
    params: &mut TransitionParams,
    section: &TransitionSection,
    prefix: &str,
) -> Result<()> {
    overwrite(&mut params.mode, section.mode);
    apply_tween(
        &mut params.tween,
        section.duration_ms,
        section.easing,
        &format!("{prefix}transition"),
    )
}

/// The one place `duration-ms` and `easing` are named, since every animated section
/// spells them the same way and means the same thing by them.
fn apply_tween(
    params: &mut Tween,
    duration_ms: Option<u32>,
    easing: Option<Easing>,
    path: &str,
) -> Result<()> {
    if let Some(millis) = duration_ms {
        if millis > MAX_DURATION_MS {
            let key = format!("{path}.duration-ms");
            return Err(Invalid::new(key, format!("expected at most {MAX_DURATION_MS} ms")));
        }
        params.duration = seconds_from_millis(millis);
    }
    overwrite(&mut params.easing, easing);
    Ok(())
}

fn overwrite<T>(slot: &mut T, value: Option<T>) {
    if let Some(value) = value {
        *slot = value;
    }
}

fn set_ratio(slot: &mut f32, value: Option<f32>, key: &str) -> Result<()> {
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Invalid::new(key, "expected a number in 0..=1"));
        }
        *slot = value;
    }
    Ok(())
}

fn set_bounded(slot: &mut u32, value: Option<u32>, lo: u32, hi: u32, key: &str) -> Result<()> {
    if let Some(value) = value {
        if !(lo..=hi).contains(&value) {
            return Err(Invalid::new(key, format!("expected an integer in {lo}..={hi}")));
        }
        *slot = value;
    }
    Ok(())
}

/// Expands a leading `~` and anchors anything still relative to the config file's
/// directory, since a daemon's working directory is not something a user can reason about.
fn resolve_path(raw: &str, ctx: &Context<'_>, key: &str) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        return Err(Invalid::new(key, "must not be empty"));
    }
    let expanded = match raw.strip_prefix('~') {
        Some("") => home(ctx, key)?.to_path_buf(),
        Some(rest) => match rest.strip_prefix('/') {
            Some(rest) => home(ctx, key)?.join(rest),
            None => PathBuf::from(raw),
        },
        None => PathBuf::from(raw),
    };
    if expanded.is_absolute() { Ok(expanded) } else { Ok(ctx.base_dir.join(expanded)) }
}

fn home<'a>(ctx: &Context<'a>, key: &str) -> Result<&'a Path> {
    ctx.home.ok_or_else(|| Invalid::new(key, "HOME is unset, so `~` cannot be expanded"))
}

#[cfg(test)]
mod tests {
    use domain::{Easing, Layer, Rgba, TransitionMode};

    use super::*;

    const NAMESPACE: &str = "injected-namespace";

    fn context() -> Context<'static> {
        Context {
            default_namespace: NAMESPACE,
            base_dir: Path::new("/etc/xdg/somewhere"),
            home: Some(Path::new("/home/tester")),
        }
    }

    fn parse(text: &str) -> Result<Config> {
        let file: ConfigFile = toml::from_str(text).expect("test input should be valid TOML");
        resolve(&file, &context())
    }

    fn dp1() -> OutputId {
        OutputId::new("DP-1")
    }

    fn wallpaper_of(config: &Config, output: &OutputId) -> Option<PathBuf> {
        config.for_output(output).fallback.as_ref().map(|w| w.path().to_path_buf())
    }

    #[test]
    fn an_empty_file_yields_the_domain_defaults() {
        let config = parse("").unwrap();
        assert_eq!(config.global, OutputParams::default());
        assert_eq!(config.surface.namespace, NAMESPACE);
        assert_eq!(config.surface.layer, Layer::Background);
    }

    /// `config.example.toml` says every key is shown with its default. Nothing but this
    /// stops that from quietly becoming false the next time a default moves.
    #[test]
    fn the_example_config_states_the_real_defaults() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.example.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

        let config = parse(&text).expect("the example config should be valid");

        assert_eq!(
            config.global,
            OutputParams::default(),
            "config.example.toml has drifted from the defaults in `domain`"
        );
        assert_eq!(config.surface.namespace, NAMESPACE, "the example should not set a namespace");
        assert_eq!(config.surface.layer, Layer::default());
    }

    #[test]
    fn the_namespace_falls_back_to_the_injected_one() {
        let config = parse("[general]\nnamespace = \"custom\"\n").unwrap();
        assert_eq!(config.surface.namespace, "custom");
    }

    #[test]
    fn an_empty_namespace_is_rejected() {
        let error = parse("[general]\nnamespace = \"  \"\n").unwrap_err();
        assert_eq!(error.key, "general.namespace");
    }

    #[test]
    fn global_values_reach_every_output() {
        let config = parse(
            r##"
            [blur]
            radius = 48
            tint = "#112233"
            [scroll.vertical]
            travel = 0.25
            easing = "in-out-quad"
            "##,
        )
        .unwrap();

        let params = config.for_output(&dp1());
        assert_eq!(params.blur.radius, 48);
        assert_eq!(params.blur.tint, Rgba::from_bytes([0x11, 0x22, 0x33, 0xff]));
        assert_eq!(params.scroll.vertical.travel, 0.25);
        assert_eq!(params.scroll.vertical.tween.easing, Easing::InOutQuad);
    }

    #[test]
    fn durations_become_seconds() {
        let config = parse("[blur]\nduration-ms = 250\n").unwrap();
        assert_eq!(config.global.blur.tween.duration, 0.25);
    }

    #[test]
    fn the_scroll_axes_carry_their_own_animation() {
        let config = parse(
            r#"
            [scroll.vertical]
            duration-ms = 250
            easing = "linear"

            [scroll.horizontal]
            enabled = true
            duration-ms = 600
            easing = "out-quint"
            "#,
        )
        .unwrap();

        let scroll = config.global.scroll;
        assert_eq!(scroll.vertical.tween.duration, 0.25);
        assert_eq!(scroll.vertical.tween.easing, Easing::Linear);
        assert!(scroll.horizontal.enabled);
        assert_eq!(scroll.horizontal.tween.duration, 0.6);
        assert_eq!(scroll.horizontal.tween.easing, Easing::OutQuint);
    }

    #[test]
    fn one_scroll_axis_leaves_the_other_at_its_default() {
        let config =
            parse("[scroll.horizontal]\nduration-ms = 600\neasing = \"linear\"\n").unwrap();
        let defaults = ScrollParams::default();

        let scroll = config.global.scroll;
        assert_eq!(scroll.horizontal.tween.duration, 0.6);
        assert_eq!(scroll.horizontal.tween.easing, Easing::Linear);
        assert_eq!(scroll.vertical, defaults.vertical);
    }

    #[test]
    fn the_horizontal_axis_is_disabled_by_default() {
        let defaults = ScrollParams::default();
        assert!(defaults.vertical.enabled);
        assert!(!defaults.horizontal.enabled);
    }

    #[test]
    fn an_output_table_inherits_every_key_it_omits() {
        let config = parse(
            r#"
            [blur]
            radius = 48
            downscale = 2
            [output."DP-1"]
            blur.radius = 16
            "#,
        )
        .unwrap();

        let params = config.for_output(&dp1());
        assert_eq!(params.blur.radius, 16, "the override should win");
        assert_eq!(params.blur.downscale, 2, "the rest should be inherited");
    }

    #[test]
    fn an_output_table_touches_nothing_else() {
        let config = parse(
            r#"
            [blur]
            radius = 48
            [output."DP-1"]
            blur.radius = 16
            "#,
        )
        .unwrap();

        assert_eq!(config.global.blur.radius, 48);
        assert_eq!(config.for_output(&OutputId::new("eDP-1")).blur.radius, 48);
    }

    #[test]
    fn outputs_may_carry_their_own_wallpaper() {
        let config = parse(
            r#"
            [wallpaper]
            fallback = "/srv/shared.png"
            [output."eDP-1"]
            wallpaper.fallback = "/srv/laptop.png"
            "#,
        )
        .unwrap();

        assert_eq!(wallpaper_of(&config, &dp1()), Some(PathBuf::from("/srv/shared.png")));
        assert_eq!(
            wallpaper_of(&config, &OutputId::new("eDP-1")),
            Some(PathBuf::from("/srv/laptop.png"))
        );
    }

    #[test]
    fn a_leading_tilde_expands_to_home() {
        let config = parse("[wallpaper]\nfallback = \"~/pictures/wall.png\"\n").unwrap();
        assert_eq!(
            wallpaper_of(&config, &dp1()),
            Some(PathBuf::from("/home/tester/pictures/wall.png"))
        );
    }

    #[test]
    fn a_relative_path_anchors_to_the_config_directory() {
        let config = parse("[wallpaper]\nfallback = \"wall.png\"\n").unwrap();
        assert_eq!(
            wallpaper_of(&config, &dp1()),
            Some(PathBuf::from("/etc/xdg/somewhere/wall.png"))
        );
    }

    #[test]
    fn an_empty_wallpaper_fallback_is_rejected() {
        assert_eq!(parse("[wallpaper]\nfallback = \"\"\n").unwrap_err().key, "wallpaper.fallback");
    }

    #[test]
    fn out_of_range_values_name_the_offending_key() {
        let cases = [
            ("[scroll.vertical]\ntravel = 1.5\n", "scroll.vertical.travel"),
            ("[scroll.horizontal]\nduration-ms = 999999\n", "scroll.horizontal.duration-ms"),
            ("[blur]\nradius = 9999\n", "blur.radius"),
            ("[blur]\ndownscale = 0\n", "blur.downscale"),
            ("[blur]\ntint-opacity = -0.1\n", "blur.tint-opacity"),
            ("[overview]\ncrop-ratio = 0.0\n", "overview.crop-ratio"),
            ("[transition]\nduration-ms = 999999\n", "transition.duration-ms"),
        ];
        for (text, expected) in cases {
            let error = parse(text).expect_err(text);
            assert_eq!(error.key, expected);
        }
    }

    #[test]
    fn an_invalid_value_under_an_output_names_the_full_path() {
        let error = parse("[output.\"DP-1\"]\nblur.radius = 9999\n").unwrap_err();
        assert_eq!(error.key, "output.\"DP-1\".blur.radius");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        for text in [
            "[blur]\nradius = 1\nradiuss = 2\n",
            "[nonsense]\nkey = 1\n",
            "[output.\"DP-1\"]\ngeneral.namespace = \"x\"\n",
        ] {
            assert!(toml::from_str::<ConfigFile>(text).is_err(), "{text} should be rejected");
        }
    }

    #[test]
    fn an_unknown_easing_lists_the_accepted_ones() {
        let error =
            toml::from_str::<ConfigFile>("[scroll.vertical]\neasing = \"wobble\"\n").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("out-cubic"), "{message}");
    }

    #[test]
    fn the_transition_mode_parses_even_though_it_is_not_implemented() {
        let config = parse("[transition]\nmode = \"fade\"\nduration-ms = 800\n").unwrap();
        assert_eq!(config.global.transition.mode, TransitionMode::Fade);
        assert_eq!(config.global.transition.tween.duration, 0.8);
    }
}
