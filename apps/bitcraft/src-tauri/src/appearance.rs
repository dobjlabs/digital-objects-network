use std::{fs, path::Path};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tauri::{Manager, window::Color};

const SETTINGS_FILE: &str = "appearance.json";

/// The theme the player picks in the game's Settings. The game page keeps
/// its own copy in its origin's browser storage; this one styles the window
/// frames and the desktop shell's pages, which that storage does not reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    Light,
    Dark,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    appearance: Appearance,
}

impl Appearance {
    fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn theme(self) -> tauri::Theme {
        match self {
            Self::Light => tauri::Theme::Light,
            Self::Dark => tauri::Theme::Dark,
        }
    }

    /// The pages' background in this theme, shown until a page paints.
    pub fn background(self) -> Color {
        match self {
            Self::Light => Color(0xff, 0xff, 0xff, 0xff),
            Self::Dark => Color(0x1e, 0x1e, 0x1e, 0xff),
        }
    }

    /// Runs before each page's own scripts.
    pub fn initialization_script(self) -> String {
        format!("window.bitcraftAppearance = \"{}\";", self.name())
    }

    fn restyle_script(self) -> String {
        format!(
            "window.bitcraftAppearance = \"{name}\"; document.documentElement.dataset.theme = \"{name}\";",
            name = self.name()
        )
    }
}

/// A missing or unreadable file leaves the default theme.
pub fn load(home: &Path) -> Appearance {
    fs::read(home.join(SETTINGS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
        .unwrap_or_default()
        .appearance
}

fn save(home: &Path, appearance: Appearance) -> Result<()> {
    fs::write(
        home.join(SETTINGS_FILE),
        serde_json::to_vec_pretty(&Saved { appearance })?,
    )?;
    Ok(())
}

pub fn current(app: &tauri::AppHandle) -> Appearance {
    *app.state::<crate::Desktop>().appearance.lock().unwrap()
}

/// Saves the player's pick and restyles every open window.
#[tauri::command]
pub fn set_appearance(app: tauri::AppHandle, appearance: Appearance) {
    let desktop = app.state::<crate::Desktop>();
    *desktop.appearance.lock().unwrap() = appearance;
    if let Err(error) = save(&desktop.home, appearance) {
        eprintln!("Could not save the theme: {error:#}");
    }
    app.set_theme(Some(appearance.theme()));
    for window in app.webview_windows().values() {
        let _ = window.set_background_color(Some(appearance.background()));
        let _ = window.eval(appearance.restyle_script());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_is_light_until_one_is_saved_in_the_home() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(load(home.path()), Appearance::Light);
        save(home.path(), Appearance::Dark).unwrap();
        assert_eq!(load(home.path()), Appearance::Dark);
        save(home.path(), Appearance::Light).unwrap();
        assert_eq!(load(home.path()), Appearance::Light);
        fs::write(home.path().join(SETTINGS_FILE), b"invalid").unwrap();
        assert_eq!(load(home.path()), Appearance::Light);
    }

    #[test]
    fn the_page_can_only_pick_a_known_theme() {
        for (argument, appearance) in [
            ("\"light\"", Some(Appearance::Light)),
            ("\"dark\"", Some(Appearance::Dark)),
        ] {
            assert_eq!(
                serde_json::from_str::<Appearance>(argument).ok(),
                appearance
            );
        }
        for argument in ["\"system\"", "\"Dark\"", "1", "null"] {
            assert!(
                serde_json::from_str::<Appearance>(argument).is_err(),
                "{argument}"
            );
        }
    }
}
