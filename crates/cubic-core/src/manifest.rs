//! The project manifest: `game.toml`, the one file that describes a game.
//!
//! A project is a directory with a `game.toml` at its root, a `Cargo.toml` beside
//! it and `src/` inside. The manifest is what the engine is told about a project
//! before the game runs — its name, the window it opens in, the rate its
//! simulation ticks at, the features that get built into it — and it is the same
//! file `cubic-cli` finds a project by and the editor opens.
//!
//! ```toml
//! [game]
//! name = "demo"
//! version = "0.1.0"
//! tick_hz = 60
//! icon = "assets/icon.png"
//!
//! [window]
//! title = "demo"
//! width = 960
//! height = 540
//! resizable = true
//! clear_color = "#121721"
//!
//! [features]
//! web = false
//! ```
//!
//! # Every field but the name has a default
//!
//! A manifest can be as short as its author wants. The example above is the
//! whole file a generated project ships with; a manifest of nothing but a name
//! opens a 960x540 window titled after it and ticks at 60 Hz. Unknown keys are
//! an error rather than a shrug — a misspelled `widht` should not quietly mean
//! "the default" — and so is a window that cannot be opened.
//!
//! # The manifest is compiled in
//!
//! A game does not read its manifest from disk at runtime; it embeds it and the
//! tree it was read from, so the settings a build ships are the settings that ran.
//! A changed `game.toml` rebuilds the game, because the file is a build input:
//!
//! ```ignore
//! // in a game's own crate, where `../game.toml` is the project root's manifest
//! let manifest = ProjectManifest::embedded(include_str!("../game.toml"));
//! ```
//!
//! # A bad tick rate is not fatal
//!
//! `game.tick_hz` is the one value that is *not* validated. Zero, a negative
//! number or `NaN` all parse, and the host's fixed-tick clock substitutes its
//! own default: a project with a nonsense rate runs at 60 Hz instead of
//! refusing to start.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, de};

use crate::render::Rgba;

/// Window width used when `[window]` does not give one.
pub const DEFAULT_WINDOW_WIDTH: u32 = 960;

/// Window height used when `[window]` does not give one.
pub const DEFAULT_WINDOW_HEIGHT: u32 = 540;

/// Simulation rate used when `game.tick_hz` is missing — and when it is
/// nonsense, since the fixed-tick clock sanitizes rather than fails.
pub const DEFAULT_TICK_HZ: f32 = 60.0;

/// The backdrop a project gets when `[window]` does not name one.
pub const DEFAULT_CLEAR_COLOR: Rgba = Rgba::rgb(0.07, 0.09, 0.13);

/// A parsed `game.toml`.
///
/// See the [module docs](self) for the file format and the rules it enforces.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectManifest {
    /// Who the project is: its crate name, version, tick rate and icon.
    pub game: GameSpec,
    /// The window the project opens.
    pub window: WindowSpec,
    /// Extra engine features to build in, by name. A flag that is absent or
    /// `false` is off, so a manifest can list the whole menu and enable what it
    /// currently uses.
    pub features: BTreeMap<String, bool>,
}

/// The `[game]` table: identity and simulation rate.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GameSpec {
    /// Crate name, matching the `package.name` in the project's `Cargo.toml`.
    pub name: String,
    /// Crate version, mirroring the same file.
    pub version: String,
    /// Steps per second the simulation runs at. Not validated — see the
    /// [module docs](self).
    pub tick_hz: f32,
    /// Window/taskbar icon, as a path relative to the project root.
    pub icon: Option<String>,
}

impl Default for GameSpec {
    fn default() -> Self {
        Self {
            name: "game".to_string(),
            version: "0.1.0".to_string(),
            tick_hz: DEFAULT_TICK_HZ,
            icon: None,
        }
    }
}

/// The `[window]` table: the frame the game draws into.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct WindowSpec {
    /// Title bar text.
    pub title: String,
    /// Logical width at open, in pixels.
    pub width: u32,
    /// Logical height at open, in pixels.
    pub height: u32,
    /// Whether the user may resize the window. The viewport follows the window
    /// either way; this only decides whether they get the chance.
    pub resizable: bool,
    /// Backdrop for frames the game does not clear itself.
    #[serde(deserialize_with = "de_color")]
    pub clear_color: Rgba,
}

impl Default for WindowSpec {
    fn default() -> Self {
        Self {
            title: String::new(),
            width: DEFAULT_WINDOW_WIDTH,
            height: DEFAULT_WINDOW_HEIGHT,
            resizable: true,
            clear_color: DEFAULT_CLEAR_COLOR,
        }
    }
}

impl ProjectManifest {
    /// Parse a manifest, rejecting anything that does not describe a runnable
    /// project.
    ///
    /// `text` is the whole file. Resolution is not the caller's problem: the
    /// game embeds the manifest text (see the [module docs](self)) and the CLI
    /// reads it off disk, but both hand the same bytes here.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let manifest: Self = toml::from_str(text).map_err(ManifestError::Syntax)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Parse a manifest that was embedded at compile time.
    ///
    /// A malformed embedded manifest is a build mistake in the project itself,
    /// not a runtime condition a game could handle, so this panics with the
    /// parse error rather than returning it.
    pub fn embedded(text: &str) -> Self {
        Self::parse(text).unwrap_or_else(|error| panic!("game.toml is not usable: {error}"))
    }

    /// The window's title bar text, defaulting to the project's name.
    ///
    /// A manifest may leave `window.title` empty to say "whatever the project is
    /// called", which keeps a rename from having to touch two keys.
    pub fn window_title(&self) -> &str {
        if self.window.title.is_empty() {
            &self.game.name
        } else {
            &self.window.title
        }
    }

    /// The names of the features this project wants built in.
    ///
    /// Sorted, because the source is a [`BTreeMap`] and a build command built
    /// from it should not depend on how a manifest happened to be written.
    pub fn enabled_features(&self) -> impl Iterator<Item = &str> {
        self.features
            .iter()
            .filter(|(_, on)| **on)
            .map(|(name, _)| name.as_str())
    }

    /// Reject a manifest that parses but cannot be run.
    ///
    /// Split out from [`parse`](Self::parse) so a host that builds a manifest
    /// in memory can check it the same way.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.game.name.trim().is_empty() {
            return Err(ManifestError::Invalid(
                "`game.name` is required and cannot be blank".to_string(),
            ));
        }
        if self.window.width == 0 || self.window.height == 0 {
            return Err(ManifestError::Invalid(format!(
                "`window` is {}x{}; both sides must be at least 1x1",
                self.window.width, self.window.height
            )));
        }
        if self.game.icon.as_deref() == Some("") {
            return Err(ManifestError::Invalid(
                "`game.icon` is empty; leave it out for no icon".to_string(),
            ));
        }
        Ok(())
    }
}

/// Why a manifest was refused.
#[derive(Debug)]
pub enum ManifestError {
    /// The file is not TOML, or a value has the wrong shape.
    Syntax(toml::de::Error),
    /// The file parses but does not describe something runnable.
    Invalid(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(error) => write!(f, "game.toml is not valid: {error}"),
            Self::Invalid(message) => write!(f, "game.toml: {message}"),
        }
    }
}

impl std::error::Error for ManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syntax(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

/// Read a `#rrggbb`/`#rrggbbaa` color, with or without the leading `#`.
///
/// A manifest is edited by hand, so a color is a hex string rather than four
/// floats; this is the whole of the conversion. Components are scaled into
/// `0.0..=1.0` on the way, which is what [`Rgba`] holds.
fn parse_hex_color(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |index: usize| u8::from_str_radix(&digits[index..index + 2], 16).ok();
    let component = |index: usize| byte(index).map(|v| f32::from(v) / 255.0);
    match digits.len() {
        6 => Some(Rgba::rgb(component(0)?, component(2)?, component(4)?)),
        8 => Some(Rgba::new(
            component(0)?,
            component(2)?,
            component(4)?,
            component(6)?,
        )),
        _ => None,
    }
}

/// `#[serde]` hook for [`WindowSpec::clear_color`].
fn de_color<'de, D>(deserializer: D) -> Result<Rgba, D::Error>
where
    D: Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    parse_hex_color(&text).ok_or_else(|| {
        de::Error::custom(format!(
            "`{text}` is not a hex color; expected \"#rrggbb\" or \"#rrggbbaa\""
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The manifest a generated project ships, exactly as the module docs show
    /// it. Parsing the documented file is what keeps the two honest.
    const DOCUMENTED: &str = r##"
[game]
name = "demo"
version = "0.1.0"
tick_hz = 60
icon = "assets/icon.png"

[window]
title = "demo"
width = 960
height = 540
resizable = true
clear_color = "#121721"

[features]
web = false
"##;

    #[test]
    fn the_documented_manifest_parses() {
        let manifest = ProjectManifest::parse(DOCUMENTED).expect("documented manifest is valid");
        assert_eq!(manifest.game.name, "demo");
        assert_eq!(manifest.window_title(), "demo");
        assert_eq!(
            manifest.window.clear_color,
            parse_hex_color("#121721").unwrap()
        );
        assert_eq!(manifest.enabled_features().count(), 0);
    }

    /// The minimum: a name and nothing else still describes a runnable project.
    #[test]
    fn a_manifest_of_only_a_name_takes_every_default() {
        let manifest = ProjectManifest::parse("[game]\nname = \"solo\"\n").unwrap();
        assert_eq!(manifest.game.name, "solo");
        assert_eq!(manifest.game.version, "0.1.0");
        assert_eq!(manifest.game.tick_hz, DEFAULT_TICK_HZ);
        assert_eq!(manifest.game.icon, None);
        assert_eq!(manifest.window.width, DEFAULT_WINDOW_WIDTH);
        assert_eq!(manifest.window.height, DEFAULT_WINDOW_HEIGHT);
        assert!(manifest.window.resizable);
        assert_eq!(manifest.window.clear_color, DEFAULT_CLEAR_COLOR);
    }

    /// An empty file is still a project — the name falls back, so a
    /// hand-stubbed manifest is not a special case to remember.
    #[test]
    fn an_empty_manifest_is_still_a_project() {
        let manifest = ProjectManifest::parse("").unwrap();
        assert_eq!(manifest.game.name, "game");
        assert_eq!(manifest.window_title(), "game");
    }

    #[test]
    fn every_field_can_be_overridden() {
        let manifest = ProjectManifest::parse(
            r##"
[game]
name = "tuned"
version = "2.1.0"
tick_hz = 120
icon = "assets/art/window.png"

[window]
title = "Tuned"
width = 1280
height = 720
resizable = false
clear_color = "#ff8800"
"##,
        )
        .unwrap();

        assert_eq!(manifest.game.name, "tuned");
        assert_eq!(manifest.game.version, "2.1.0");
        assert_eq!(manifest.game.tick_hz, 120.0);
        assert_eq!(manifest.game.icon.as_deref(), Some("assets/art/window.png"));
        assert_eq!(manifest.window_title(), "Tuned");
        assert_eq!(manifest.window.width, 1280);
        assert_eq!(manifest.window.height, 720);
        assert!(!manifest.window.resizable);
        let color = manifest.window.clear_color;
        assert!((color.r - 1.0).abs() < 1e-6);
        assert!((color.g - 136.0 / 255.0).abs() < 1e-6);
        assert!(color.b < 1e-6);
        assert!((color.a - 1.0).abs() < 1e-6);
    }

    /// An untitled window takes the project's name, so renaming the project does
    /// not leave a stale title behind.
    #[test]
    fn an_untitled_window_is_titled_after_the_project() {
        let manifest = ProjectManifest::parse("[game]\nname = \"platformer\"\n").unwrap();
        assert_eq!(manifest.window.title, "");
        assert_eq!(manifest.window_title(), "platformer");
    }

    #[test]
    fn features_are_read_as_flags_and_reported_sorted() {
        let manifest = ProjectManifest::parse(
            r#"
[game]
name = "demo"

[features]
web = true
alpha = false
beta = true
"#,
        )
        .unwrap();

        assert_eq!(
            manifest.enabled_features().collect::<Vec<_>>(),
            ["beta", "web"]
        );
        assert_eq!(manifest.features.get("alpha"), Some(&false));
        assert_eq!(manifest.features.get("missing"), None);
    }

    /// A misspelled key silently meaning "the default" is the failure mode worth
    /// ruling out.
    #[test]
    fn unknown_keys_are_refused() {
        for bad in [
            "[window]\nwidht = 640\n",
            "[game]\nname = \"demo\"\ntick = 60\n",
            "[rendering]\nweb = true\n",
            "[features]\nwgpu = \"yes\"\n",
        ] {
            assert!(ProjectManifest::parse(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_malformed_file_reports_where_it_broke() {
        let error = ProjectManifest::parse("[game\nname = \"demo\"").unwrap_err();
        let message = error.to_string();
        assert!(message.starts_with("game.toml is not valid"), "{message}");
        // The `toml` error carries the line, so the message points at it.
        assert!(message.contains("line"), "{message}");
    }

    #[test]
    fn a_window_that_cannot_be_opened_is_refused() {
        for bad in [
            "[window]\nwidth = 0\n",
            "[window]\nheight = 0\n",
            "[window]\nwidth = 640\nheight = 0\n",
        ] {
            let error = ProjectManifest::parse(bad).unwrap_err();
            assert!(matches!(error, ManifestError::Invalid(_)), "{bad:?}");
        }
    }

    #[test]
    fn a_project_without_a_name_is_refused() {
        for bad in ["[game]\nname = \"\"\n", "[game]\nname = \"   \"\n"] {
            let error = ProjectManifest::parse(bad).unwrap_err();
            assert!(error.to_string().contains("game.name"), "{error}");
        }
    }

    #[test]
    fn an_empty_icon_path_is_refused_rather_than_used() {
        let error = ProjectManifest::parse("[game]\nname = \"demo\"\nicon = \"\"\n").unwrap_err();
        assert!(error.to_string().contains("game.icon"), "{error}");
    }

    /// The documented promise: a nonsense rate parses, and the host's clock is
    /// what substitutes a sane one.
    #[test]
    fn a_nonsense_tick_rate_is_accepted_rather_than_fatal() {
        for bad in ["0", "-1", "1e40"] {
            let manifest =
                ProjectManifest::parse(&format!("[game]\nname = \"demo\"\ntick_hz = {bad}\n"))
                    .expect("tick rate must not stop a project from loading");
            assert!(manifest.game.tick_hz <= 0.0 || !manifest.game.tick_hz.is_finite());
        }
    }

    #[test]
    fn colors_parse_with_and_without_the_hash_and_with_alpha() {
        let opaque = parse_hex_color("#ff0000").unwrap();
        assert_eq!(
            (opaque.r, opaque.g, opaque.b, opaque.a),
            (1.0, 0.0, 0.0, 1.0)
        );

        let bare = parse_hex_color("00ff00").unwrap();
        assert!((bare.g - 1.0).abs() < 1e-6);

        let faded = parse_hex_color("#0000ff80").unwrap();
        assert!((faded.b - 1.0).abs() < 1e-6);
        assert!((faded.a - 128.0 / 255.0).abs() < 1e-6);

        // Round-tripped through the manifest, so the deserializer is exercised
        // too rather than just the helper.
        let manifest = ProjectManifest::parse("[window]\nclear_color = \"#10203040\"\n").unwrap();
        assert!((manifest.window.clear_color.g - 32.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn an_unparseable_color_is_a_readable_error() {
        for bad in ["blue", "#12345", "#gggggg", "#", ""] {
            let error = ProjectManifest::parse(&format!("[window]\nclear_color = \"{bad}\"\n"))
                .unwrap_err();
            let message = error.to_string();
            assert!(message.contains("hex color"), "{bad:?}: {message}");
        }
    }

    /// An embedded manifest that does not compile is a bug in the project, not a
    /// condition the game can respond to.
    #[test]
    #[should_panic(expected = "game.toml is not usable")]
    fn an_unusable_embedded_manifest_panics_with_the_reason() {
        ProjectManifest::embedded("[window]\nwidth = 0\n");
    }

    #[test]
    fn a_built_manifest_validates_the_same_way_a_parsed_one_does() {
        let manifest = ProjectManifest::parse("[game]\nname = \"demo\"\n").unwrap();
        assert!(manifest.validate().is_ok());

        let mut broken = manifest.clone();
        broken.window.width = 0;
        assert!(broken.validate().is_err());
    }
}
