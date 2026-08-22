use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use domain::anim::seconds_from_millis;
use domain::params::{MAX_SHIFT, MIN_CROP_RATIO};
use domain::{
    AxisParams, BlurParams, Easing, OutputId, OutputParams, ScrollParams, SurfaceParams,
    TransitionParams, Tween, WallpaperRef, ZoomParams,
};

use crate::schema::{
    AxisSection, BlurSection, ConfigFile, OutputSection, ScrollSection, TransitionSection,
    WallpaperSection, ZoomSection,
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
    compositor: toml::Table,
    /// Only the outputs that override something, each already overlaid on the global
    /// table so that whoever reads one reads a complete section.
    compositor_per_output: BTreeMap<OutputId, toml::Table>,
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

    /// The compositor's own section, exactly as it was written.
    pub fn compositor_table(&self) -> &toml::Table {
        &self.compositor
    }

    /// Each output that says something of its own about the compositor, with the global
    /// section already underneath it.
    pub fn compositor_overrides(&self) -> impl Iterator<Item = (&OutputId, &toml::Table)> {
        self.compositor_per_output.iter()
    }
}

/// Lays one table over another, key path by key path.
///
/// Two tables under the same key merge; anything else replaces, which is how a leaf under
/// an output overrides its global twin while the rest of the section is inherited.
fn overlay(base: &toml::Table, over: &toml::Table) -> toml::Table {
    let mut merged = base.clone();
    for (key, value) in over {
        match (merged.get(key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(over)) => {
                merged.insert(key.clone(), overlay(base, over).into());
            }
            _ => {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    merged
}

/// Every section that can appear both globally and under one output.
struct Sections<'a> {
    wallpaper: &'a WallpaperSection,
    scroll: &'a ScrollSection,
    blur: &'a BlurSection,
    zoom: &'a ZoomSection,
    transition: &'a TransitionSection,
}

impl<'a> Sections<'a> {
    fn global(file: &'a ConfigFile) -> Self {
        Self {
            wallpaper: &file.wallpaper,
            scroll: &file.scroll,
            blur: &file.blur,
            zoom: &file.zoom,
            transition: &file.transition,
        }
    }

    fn output(section: &'a OutputSection) -> Self {
        Self {
            wallpaper: &section.wallpaper,
            scroll: &section.scroll,
            blur: &section.blur,
            zoom: &section.zoom,
            transition: &section.transition,
        }
    }
}

pub fn resolve(file: &ConfigFile, ctx: &Context<'_>) -> Result<Config> {
    let mut global = OutputParams::default();
    apply(&mut global, Sections::global(file), "", ctx)?;

    let mut per_output = BTreeMap::new();
    let mut compositor_per_output = BTreeMap::new();
    for (name, section) in &file.output {
        if name.is_empty() {
            return Err(Invalid::new("output", "an output table needs a connector name"));
        }
        let prefix = format!("output.{name:?}.");
        let mut params = global.clone();
        apply(&mut params, Sections::output(section), &prefix, ctx)?;
        let id = OutputId::new(name.clone());
        if !section.compositor.is_empty() {
            compositor_per_output
                .insert(id.clone(), overlay(&file.compositor, &section.compositor));
        }
        per_output.insert(id, params);
    }

    let namespace = match &file.general.namespace {
        Some(value) if value.trim().is_empty() => {
            return Err(Invalid::new("general.namespace", "must not be empty"));
        }
        Some(value) => value.clone(),
        None => ctx.default_namespace.to_owned(),
    };
    let surface = SurfaceParams { namespace, layer: file.general.layer.unwrap_or_default() };

    Ok(Config {
        surface,
        global,
        per_output,
        compositor: file.compositor.clone(),
        compositor_per_output,
    })
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
    apply_zoom(&mut params.zoom, sections.zoom, prefix)?;
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
    set_ratio(&mut params.travel, section.travel, &format!("{path}.travel"))?;
    overwrite(&mut params.invert, section.invert);
    set_cap(&mut params.max_shift, section.max_shift, &format!("{path}.max-shift"))?;
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

fn apply_zoom(params: &mut ZoomParams, section: &ZoomSection, prefix: &str) -> Result<()> {
    if let Some(value) = section.crop_ratio {
        let key = format!("{prefix}zoom.crop-ratio");
        if !value.is_finite() || !(MIN_CROP_RATIO..=1.0).contains(&value) {
            return Err(Invalid::new(key, format!("expected a number in {MIN_CROP_RATIO}..=1")));
        }
        params.crop_ratio = value;
    }
    apply_tween(&mut params.tween, section.duration_ms, section.easing, &format!("{prefix}zoom"))
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

/// Zero lifts the cap, which is how an output asks for the whole travel back once
/// something above it has capped one. Pinning an axis is `travel = 0` and only that, so a
/// zero here must not mean it too.
fn set_cap(slot: &mut Option<f32>, value: Option<f32>, key: &str) -> Result<()> {
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..=MAX_SHIFT).contains(&value) {
            return Err(Invalid::new(key, format!("expected a number in 0..={MAX_SHIFT}")));
        }
        *slot = (value > 0.0).then_some(value);
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

    /// Every example file says its keys are shown with their defaults. Nothing but this
    /// stops that from quietly becoming false the next time a default moves.
    ///
    /// Found by pattern rather than by name, since there is one example per compositor.
    #[test]
    fn the_example_configs_state_the_real_defaults() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let examples: Vec<PathBuf> = std::fs::read_dir(&root)
            .expect("the repository root should be readable")
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                path.file_name()?.to_str()?.ends_with(".example.toml").then_some(path)
            })
            .collect();
        assert!(!examples.is_empty(), "no example configuration was found to check");

        for path in examples {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{name} should be readable: {e}"));
            let config = parse(&text).unwrap_or_else(|e| panic!("{name} should be valid: {e:?}"));

            assert_eq!(
                config.global,
                OutputParams::default(),
                "{name} has drifted from the defaults in `domain`"
            );
            assert_eq!(config.surface.namespace, NAMESPACE, "{name} should not set a namespace");
            assert_eq!(config.surface.layer, Layer::default());
        }
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
            duration-ms = 600
            easing = "out-quint"
            "#,
        )
        .unwrap();

        let scroll = config.global.scroll;
        assert_eq!(scroll.vertical.tween.duration, 0.25);
        assert_eq!(scroll.vertical.tween.easing, Easing::Linear);
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
    fn a_compositor_table_is_carried_through_unread() {
        let config = parse("[compositor]\nvertical = \"workspace\"\nnonsense = 3\n").unwrap();
        let table = config.compositor_table();

        assert_eq!(table["vertical"].as_str(), Some("workspace"));
        assert_eq!(table["nonsense"].as_integer(), Some(3), "unknown keys survive the crossing");
    }

    #[test]
    fn an_output_compositor_table_is_laid_over_the_global_one() {
        let config = parse(
            r#"
            [compositor]
            vertical = "workspace"
            horizontal = "column"
            [output."DP-1".compositor]
            horizontal = "none"
            "#,
        )
        .unwrap();

        let overrides: Vec<_> = config.compositor_overrides().collect();
        assert_eq!(overrides.len(), 1, "only the output that said something");
        let (output, table) = overrides[0];
        assert_eq!(output, &dp1());
        assert_eq!(table["horizontal"].as_str(), Some("none"), "the override should win");
        assert_eq!(table["vertical"].as_str(), Some("workspace"), "the rest is inherited");
    }

    #[test]
    fn an_edit_under_one_output_shows_up_as_a_difference() {
        let global = "[compositor]\nhorizontal = \"column\"\n";
        let before = parse(global).unwrap();
        let after =
            parse(&format!("{global}[output.\"DP-1\".compositor]\nvertical = \"none\"\n")).unwrap();

        assert_eq!(before.compositor_table(), after.compositor_table(), "the global section");
        assert!(
            !before.compositor_overrides().eq(after.compositor_overrides()),
            "a reload has to notice an edit that only one output made"
        );
    }

    #[test]
    fn an_output_with_no_compositor_table_overrides_nothing() {
        let config = parse(
            r#"
            [compositor]
            horizontal = "column"
            [output."DP-1"]
            blur.radius = 16
            "#,
        )
        .unwrap();

        assert_eq!(config.compositor_overrides().count(), 0);
    }

    #[test]
    fn a_nested_compositor_key_merges_rather_than_replacing() {
        let config = parse(
            r#"
            [compositor.axes]
            vertical = "workspace"
            horizontal = "column"
            [output."DP-1".compositor.axes]
            horizontal = "none"
            "#,
        )
        .unwrap();

        let (_, table) = config.compositor_overrides().next().expect("DP-1 overrides something");
        let axes = table["axes"].as_table().expect("a table");
        assert_eq!(axes["horizontal"].as_str(), Some("none"));
        assert_eq!(axes["vertical"].as_str(), Some("workspace"), "a sibling key survives");
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
            ("[scroll.vertical]\nmax-shift = 20.0\n", "scroll.vertical.max-shift"),
            ("[scroll.horizontal]\nmax-shift = -0.1\n", "scroll.horizontal.max-shift"),
            ("[scroll.horizontal]\nduration-ms = 999999\n", "scroll.horizontal.duration-ms"),
            ("[blur]\nradius = 9999\n", "blur.radius"),
            ("[blur]\ndownscale = 0\n", "blur.downscale"),
            ("[blur]\ntint-opacity = -0.1\n", "blur.tint-opacity"),
            ("[zoom]\ncrop-ratio = 0.0\n", "zoom.crop-ratio"),
            ("[transition]\nduration-ms = 999999\n", "transition.duration-ms"),
        ];
        for (text, expected) in cases {
            let error = parse(text).expect_err(text);
            assert_eq!(error.key, expected);
        }
    }

    #[test]
    fn a_shift_cap_is_read_per_axis() {
        let config = parse("[scroll.vertical]\nmax-shift = 0.25\n").unwrap();
        assert_eq!(config.global.scroll.vertical.max_shift, Some(0.25));
        assert_eq!(
            config.global.scroll.horizontal.max_shift,
            AxisParams::default().max_shift,
            "the other axis keeps the default"
        );
    }

    /// Zero is how one monitor asks for the whole travel back, which is why it lifts the
    /// cap rather than pinning the axis the way `travel = 0` does.
    #[test]
    fn a_zero_shift_cap_lifts_it_rather_than_pinning_the_axis() {
        let config = parse(
            "[scroll.vertical]\nmax-shift = 0.25\n\n             [output.\"DP-1\".scroll.vertical]\nmax-shift = 0\n",
        )
        .unwrap();

        assert_eq!(config.for_output(&dp1()).scroll.vertical.max_shift, None);
        assert_eq!(config.for_output(&dp1()).scroll.vertical.travel, 1.0, "still moves");
        assert_eq!(config.global.scroll.vertical.max_shift, Some(0.25));
    }

    /// The property the key shape was chosen for: direction and distance are inherited
    /// apart, so an output restating one need not restate the other.
    #[test]
    fn an_output_may_change_how_far_an_axis_moves_without_changing_which_way() {
        let config = parse(
            r#"
            [scroll.vertical]
            invert = true
            [output."DP-1".scroll.vertical]
            travel = 0.5
            "#,
        )
        .unwrap();

        let axis = config.for_output(&dp1()).scroll.vertical;
        assert_eq!(axis.travel, 0.5, "the override should win");
        assert!(axis.invert, "the inversion should be inherited");
        assert!(!config.global.scroll.horizontal.invert, "the other axis is untouched");
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
    fn a_transition_carries_both_its_mode_and_its_duration() {
        // Neither value is the default, or dropping the override would still pass.
        let config = parse("[transition]\nmode = \"none\"\nduration-ms = 250\n").unwrap();
        assert_eq!(config.global.transition.mode, TransitionMode::None);
        assert_eq!(config.global.transition.tween.duration, 0.25);
    }
}
