//! The apps people come to kimchi from, and what kimchi does with each: the project files it
//! opens and writes for it, how to bring a project over and send a cut back, where its looks and
//! plugins are, its keyboard layout, and where it is installed. The first-run setup asks which one
//! a person used, `app.onboarding` lists the ones found on this computer, and
//! `docs/COMPATIBILITY.md` is written from this table.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A video editor.
    Editor,
    /// Motion graphics and compositing.
    Motion,
    /// Photos and colour.
    Photo,
    /// 3D.
    #[serde(rename = "3d")]
    ThreeD,
    /// Sound.
    Audio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Mac,
    Windows,
    Linux,
}

impl Os {
    pub const fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// A place on one system. `~` is the home folder; `%APPDATA%`, `%LOCALAPPDATA%`,
/// `%PROGRAMFILES%` and `%PROGRAMDATA%` are Windows' folders; a last part ending in `*` matches
/// every entry starting with what comes before it (`Adobe Premiere Pro*`). A path without a
/// slash is a program looked up on the `PATH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Place {
    pub os: Os,
    pub path: &'static str,
}

const fn mac(path: &'static str) -> Place {
    Place { os: Os::Mac, path }
}
const fn win(path: &'static str) -> Place {
    Place { os: Os::Windows, path }
}
const fn linux(path: &'static str) -> Place {
    Place { os: Os::Linux, path }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub id: &'static str,
    pub name: &'static str,
    pub vendor: &'static str,
    pub kind: Kind,
    /// Its keyboard layout in kimchi (`settings.shortcuts.keymap`), when kimchi has one.
    pub keymap: Option<&'static str>,
    /// `timeline::FORMATS` ids kimchi opens from it.
    pub opens: &'static [&'static str],
    /// `timeline::FORMATS` ids kimchi writes that it opens.
    pub writes: &'static [&'static str],
    /// How to bring a project from it into kimchi, step by step, in its own menu names.
    pub bring: &'static str,
    /// How to send a kimchi cut back to it.
    pub take: &'static str,
    /// Its looks, presets and LUTs, and which of them kimchi reads.
    pub looks: &'static str,
    /// Folders with its LUTs and presets (`looks.import` takes them as they are).
    pub look_folders: &'static [Place],
    /// Its plugins, and which of them run in kimchi.
    pub plugins: &'static str,
    /// Where it is installed: any one found means it is.
    pub detect: &'static [Place],
}

pub const APPS: &[App] = &[
    App {
        id: "premiere",
        name: "Premiere Pro",
        vendor: "Adobe",
        kind: Kind::Editor,
        keymap: Some("premiere"),
        opens: &["xmeml", "prproj", "edl"],
        writes: &["xmeml", "edl"],
        bring: "In Premiere Pro, select the sequence in the Project panel and choose File › Export › Final Cut Pro XML…. Open that .xml in kimchi (Home › Open from another editor). kimchi also reads a .prproj directly, for its sequences' cuts.",
        take: "In kimchi, choose File › Export project for › Premiere Pro (Final Cut Pro 7 XML). In Premiere Pro, choose File › Import and pick the .xml.",
        looks: "Lumetri Color presets (.prfpset) and the .cube LUTs in Lumetri's Input LUT and Creative Look menus: kimchi reads both.",
        look_folders: &[mac("/Library/Application Support/Adobe/Common/LUTs"), win("%PROGRAMFILES%/Adobe/Common/LUTs")],
        plugins: "Premiere Pro plugins are built on Adobe's own SDK and only run in Adobe's apps. Many plugin makers (Boris FX, Sapphire, Neat Video) also sell OpenFX versions, which run in kimchi.",
        detect: &[mac("/Applications/Adobe Premiere Pro*"), win("%PROGRAMFILES%/Adobe/Adobe Premiere Pro*")],
    },
    App {
        id: "finalcut",
        name: "Final Cut Pro",
        vendor: "Apple",
        kind: Kind::Editor,
        keymap: Some("finalcut"),
        opens: &["fcpxml"],
        writes: &["fcpxml"],
        bring: "In Final Cut Pro, select the project in the browser and choose File › Export XML…. Open the .fcpxml (or .fcpxmld) in kimchi.",
        take: "In kimchi, choose File › Export project for › Final Cut Pro (FCPXML). In Final Cut Pro, choose File › Import › XML….",
        looks: "Final Cut's Custom LUT effect uses .cube files: kimchi reads the same ones.",
        look_folders: &[],
        plugins: "FxPlug plugins only run in Final Cut Pro and Motion. Their makers often sell OpenFX versions, which run in kimchi.",
        detect: &[mac("/Applications/Final Cut Pro.app"), mac("/Applications/Final Cut Pro Trial.app")],
    },
    App {
        id: "resolve",
        name: "DaVinci Resolve",
        vendor: "Blackmagic Design",
        kind: Kind::Editor,
        keymap: Some("resolve"),
        opens: &["otio", "fcpxml", "xmeml", "edl"],
        writes: &["otio", "fcpxml", "xmeml", "edl"],
        bring: "In Resolve, select the timeline in the Media Pool and choose File › Export › Timeline…, then pick OpenTimelineIO (.otio) or FCPXML. Open that file in kimchi.",
        take: "In kimchi, choose File › Export project for › DaVinci Resolve (OpenTimelineIO). In Resolve, choose File › Import › Timeline… and pick the .otio.",
        looks: "Resolve's LUT folder holds .cube and .3dl LUTs: kimchi reads them as they are.",
        look_folders: &[
            mac("/Library/Application Support/Blackmagic Design/DaVinci Resolve/LUT"),
            win("%PROGRAMDATA%/Blackmagic Design/DaVinci Resolve/Support/LUT"),
            linux("/opt/resolve/LUT"),
        ],
        plugins: "OpenFX plugins installed for Resolve run in kimchi too (kimchi looks in the same OFX folder). Fusion macros and DCTL don't.",
        detect: &[mac("/Applications/DaVinci Resolve"), win("%PROGRAMFILES%/Blackmagic Design/DaVinci Resolve"), linux("/opt/resolve")],
    },
    App {
        id: "capcut",
        name: "CapCut",
        vendor: "ByteDance",
        kind: Kind::Editor,
        keymap: Some("capcut"),
        opens: &["capcut"],
        writes: &[],
        bring: "Open the project's draft folder in kimchi (CapCut's Settings › Drafts location shows where they are). Drafts from recent CapCut versions are encrypted and can't be read: export the video from CapCut instead.",
        take: "CapCut doesn't open other editors' projects: export the video from kimchi and import it in CapCut.",
        looks: "CapCut's filters are its own; LUTs (.cube) imported into CapCut work in kimchi too.",
        look_folders: &[],
        plugins: "CapCut has no plugins.",
        detect: &[mac("/Applications/CapCut.app"), win("%LOCALAPPDATA%/CapCut")],
    },
    App {
        id: "imovie",
        name: "iMovie",
        vendor: "Apple",
        kind: Kind::Editor,
        keymap: Some("imovie"),
        opens: &[],
        writes: &[],
        bring: "iMovie projects can't be read outside iMovie. Choose File › Send Movie to Final Cut Pro, export FCPXML there, and open it in kimchi; or share the movie as a file and import it.",
        take: "iMovie doesn't open other editors' projects: export the video from kimchi and import it in iMovie.",
        looks: "iMovie's filters are its own.",
        look_folders: &[],
        plugins: "iMovie has no plugins.",
        detect: &[mac("/Applications/iMovie.app")],
    },
    App {
        id: "avid",
        name: "Media Composer",
        vendor: "Avid",
        kind: Kind::Editor,
        keymap: Some("avid"),
        opens: &["edl"],
        writes: &["edl"],
        bring: "In Media Composer, open Tools › List Tool, choose CMX 3600 and save an EDL of the sequence. Open the .edl in kimchi; the media is found by its file name (point kimchi at the folder if it asks).",
        take: "In kimchi, choose File › Export project for › Avid Media Composer (EDL), then import the EDL in Media Composer.",
        looks: "Media Composer's LUTs are .cube files: kimchi reads them.",
        look_folders: &[],
        plugins: "AVX plugins only run in Media Composer.",
        detect: &[mac("/Applications/Avid Media Composer"), win("%PROGRAMFILES%/Avid/Avid Media Composer")],
    },
    App {
        id: "vegas",
        name: "VEGAS Pro",
        vendor: "MAGIX",
        kind: Kind::Editor,
        keymap: Some("vegas"),
        opens: &["xmeml", "edl"],
        writes: &["xmeml", "edl"],
        bring: "In VEGAS Pro, choose File › Export › Final Cut Pro 7 / DaVinci Resolve (*.xml)…, and open that .xml in kimchi.",
        take: "In kimchi, choose File › Export project for › VEGAS Pro (Final Cut Pro 7 XML), then File › Import › Final Cut Pro 7 / DaVinci Resolve (*.xml) in VEGAS.",
        looks: "VEGAS's LUT filter uses .cube files: kimchi reads them.",
        look_folders: &[],
        plugins: "OpenFX plugins installed for VEGAS run in kimchi too.",
        detect: &[win("%PROGRAMFILES%/VEGAS/VEGAS Pro*")],
    },
    App {
        id: "kdenlive",
        name: "Kdenlive",
        vendor: "KDE",
        kind: Kind::Editor,
        keymap: Some("kdenlive"),
        opens: &["kdenlive", "mlt", "otio"],
        writes: &["kdenlive", "mlt", "otio"],
        bring: "Open the .kdenlive file in kimchi: tracks, clips, transitions, titles and frei0r filters come along.",
        take: "In kimchi, choose File › Export project for › Kdenlive, and open the .kdenlive file in Kdenlive.",
        looks: "Kdenlive's LUT filter takes .cube files and its Hald CLUT filter images: kimchi reads both.",
        look_folders: &[],
        plugins: "Kdenlive's frei0r effects run in kimchi when the frei0r plugins are installed (the frei0r-plugins package).",
        detect: &[linux("kdenlive"), linux("/var/lib/flatpak/app/org.kde.kdenlive"), linux("~/.local/share/flatpak/app/org.kde.kdenlive"), mac("/Applications/kdenlive.app"), win("%PROGRAMFILES%/kdenlive")],
    },
    App {
        id: "shotcut",
        name: "Shotcut",
        vendor: "Meltytech",
        kind: Kind::Editor,
        keymap: Some("shotcut"),
        opens: &["mlt"],
        writes: &["mlt"],
        bring: "Open the .mlt project in kimchi: tracks, clips, transitions and frei0r filters come along.",
        take: "In kimchi, choose File › Export project for › Shotcut (MLT XML), and open it in Shotcut.",
        looks: "Shotcut's LUT filter takes .cube files: kimchi reads them.",
        look_folders: &[],
        plugins: "Shotcut's frei0r filters run in kimchi when the frei0r plugins are installed.",
        detect: &[linux("shotcut"), linux("/var/lib/flatpak/app/org.shotcut.Shotcut"), mac("/Applications/Shotcut.app"), win("%PROGRAMFILES%/Shotcut")],
    },
    App {
        id: "openshot",
        name: "OpenShot",
        vendor: "OpenShot Studios",
        kind: Kind::Editor,
        keymap: None,
        opens: &["openshot"],
        writes: &["openshot"],
        bring: "Open the .osp project in kimchi: clips, their keyframes, transitions and titles come along.",
        take: "In kimchi, choose File › Export project for › OpenShot, and open the .osp in OpenShot.",
        looks: "OpenShot's effects are its own.",
        look_folders: &[],
        plugins: "OpenShot has no plugins.",
        detect: &[linux("openshot-qt"), mac("/Applications/OpenShot Video Editor.app"), win("%PROGRAMFILES%/OpenShot Video Editor")],
    },
    App {
        id: "aftereffects",
        name: "After Effects",
        vendor: "Adobe",
        kind: Kind::Motion,
        keymap: None,
        opens: &[],
        writes: &[],
        bring: "After Effects projects can't be read outside After Effects. Render the composition (ProRes 4444 keeps transparency) and import it; for new work, kimchi's Motion Studio has layers, keyframes, expressions, masks, effects and 3D.",
        take: "Export from kimchi as ProRes 4444 or a PNG sequence and import it in After Effects.",
        looks: "Lumetri presets (.prfpset) and .cube LUTs work in kimchi too.",
        look_folders: &[],
        plugins: "After Effects plugins are built on Adobe's own SDK and only run in Adobe's apps; many also exist as OpenFX, which runs in kimchi.",
        detect: &[mac("/Applications/Adobe After Effects*"), win("%PROGRAMFILES%/Adobe/Adobe After Effects*")],
    },
    App {
        id: "nuke",
        name: "Nuke",
        vendor: "Foundry",
        kind: Kind::Motion,
        keymap: None,
        opens: &["otio"],
        writes: &["otio"],
        bring: "Export the timeline from Nuke Studio or Hiero as OpenTimelineIO and open it in kimchi.",
        take: "In kimchi, export the project as OpenTimelineIO and import it in Nuke Studio or Hiero.",
        looks: "Nuke's .cube, .3dl, .csp, .spi1d and .spi3d LUTs work in kimchi as they are.",
        look_folders: &[],
        plugins: "OpenFX plugins installed for Nuke run in kimchi too; Nuke's own NDK plugins don't.",
        detect: &[mac("/Applications/Nuke*"), win("%PROGRAMFILES%/Nuke*"), linux("/usr/local/Nuke*")],
    },
    App {
        id: "natron",
        name: "Natron",
        vendor: "Natron",
        kind: Kind::Motion,
        keymap: None,
        opens: &[],
        writes: &[],
        bring: "Render the Natron project and import the result.",
        take: "Export from kimchi as a PNG or EXR sequence and read it in Natron.",
        looks: "Natron's LUT files work in kimchi as they are.",
        look_folders: &[],
        plugins: "Natron's OpenFX plugins (openfx-misc, openfx-io, openfx-arena) run in kimchi.",
        detect: &[linux("natron"), linux("Natron"), mac("/Applications/Natron.app"), win("%PROGRAMFILES%/Natron*")],
    },
    App {
        id: "lightroom",
        name: "Lightroom and Camera Raw",
        vendor: "Adobe",
        kind: Kind::Photo,
        keymap: None,
        opens: &[],
        writes: &[],
        bring: "Your photo presets become looks: import the .xmp or .lrtemplate files (Looks › Import) and use them on any clip.",
        take: "kimchi's looks can be saved as .cube LUTs, which Lightroom's profiles don't take; use the LUT in Premiere or Photoshop.",
        looks: "Develop presets (.xmp, .lrtemplate): exposure, contrast, highlights, shadows, whites, blacks, white balance, vibrance, saturation, sharpening and vignette carry over.",
        look_folders: &[mac("~/Library/Application Support/Adobe/CameraRaw/Settings"), win("%APPDATA%/Adobe/CameraRaw/Settings")],
        plugins: "",
        detect: &[mac("/Applications/Adobe Lightroom Classic"), mac("/Applications/Adobe Lightroom CC"), win("%PROGRAMFILES%/Adobe/Adobe Lightroom Classic"), win("%PROGRAMFILES%/Adobe/Adobe Lightroom CC")],
    },
    App {
        id: "blender",
        name: "Blender",
        vendor: "Blender Foundation",
        kind: Kind::ThreeD,
        keymap: None,
        opens: &[],
        writes: &[],
        bring: "Export your models as glTF (.glb) from Blender and add them to a 3D scene in kimchi's Studio; materials, textures and animation come along.",
        take: "Render from kimchi as a PNG sequence or ProRes 4444 to use in Blender's video editor or compositor.",
        looks: "Blender's OpenColorIO LUTs (.spi1d, .spi3d, .cube) work in kimchi.",
        look_folders: &[],
        plugins: "",
        detect: &[linux("blender"), mac("/Applications/Blender.app"), win("%PROGRAMFILES%/Blender Foundation/Blender*")],
    },
    App {
        id: "audacity",
        name: "Audacity",
        vendor: "Muse Group",
        kind: Kind::Audio,
        keymap: None,
        opens: &[],
        writes: &[],
        bring: "Export the sound from Audacity (WAV or FLAC) and import it.",
        take: "Export the mix or one file per track (stems) from kimchi's export dialog.",
        looks: "",
        look_folders: &[],
        plugins: "The VST3, CLAP, LV2, LADSPA and Audio Unit effects Audacity uses also run in kimchi's mixer.",
        detect: &[linux("audacity"), mac("/Applications/Audacity.app"), win("%PROGRAMFILES%/Audacity")],
    },
];

/// The app with this id (case-insensitive).
pub fn app(id: &str) -> Option<&'static App> {
    APPS.iter().find(|a| a.id.eq_ignore_ascii_case(id.trim()))
}

/// The apps installed on this computer, in [`APPS`] order.
pub fn installed() -> Vec<&'static App> {
    APPS.iter().filter(|a| a.detect.iter().any(|p| !resolve(p).is_empty())).collect()
}

/// The folders or files a place stands for on this computer (none on other systems, or when
/// nothing is there).
pub fn resolve(place: &Place) -> Vec<PathBuf> {
    if place.os != Os::current() {
        return vec![];
    }
    if !place.path.contains('/') {
        return program(place.path).into_iter().collect();
    }
    let path = expand(place.path);
    let (dir, last) = match path.rsplit_once('/') {
        Some((d, l)) => (d.to_string(), l.to_string()),
        None => (String::new(), path.clone()),
    };
    match last.strip_suffix('*') {
        Some(prefix) => {
            let Ok(entries) = std::fs::read_dir(&dir) else { return vec![] };
            let mut found: Vec<PathBuf> =
                entries.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(prefix)).map(|e| e.path()).collect();
            found.sort();
            found
        }
        None => {
            let p = PathBuf::from(&path);
            if p.exists() { vec![p] } else { vec![] }
        }
    }
}

/// `~` and the Windows folder variables replaced, with forward slashes.
fn expand(path: &str) -> String {
    let mut out = path.to_string();
    if let Some(rest) = out.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        out = format!("{}/{rest}", home.to_string_lossy());
    }
    for (var, fallback) in [("%APPDATA%", "APPDATA"), ("%LOCALAPPDATA%", "LOCALAPPDATA"), ("%PROGRAMFILES%", "ProgramFiles"), ("%PROGRAMDATA%", "ProgramData")] {
        if out.contains(var) {
            let value = std::env::var(fallback).unwrap_or_default();
            out = out.replace(var, &value);
        }
    }
    out.replace('\\', "/")
}

/// A program on the `PATH`.
fn program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.with_extension("exe").is_file() || p.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::FORMATS;

    #[test]
    fn apps_name_real_formats_and_keymaps() {
        for a in APPS {
            for f in a.opens.iter().chain(a.writes) {
                assert!(FORMATS.iter().any(|x| x.id == *f), "{} names format {f}", a.id);
            }
        }
        for f in FORMATS {
            for a in f.apps {
                assert!(app(a).is_some(), "format {} names app {a}", f.id);
            }
        }
        for f in crate::looks::LOOK_FORMATS {
            for a in f.apps {
                assert!(app(a).is_some(), "look format {} names app {a}", f.id);
            }
        }
        let mut ids: Vec<&str> = APPS.iter().map(|a| a.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), APPS.len(), "app ids are unique");
    }

    #[test]
    fn places_expand_and_match_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Editor 2025")).unwrap();
        let pattern: &'static str = Box::leak(format!("{}/Editor*", dir.path().to_string_lossy().replace('\\', "/")).into_boxed_str());
        let found = resolve(&Place { os: Os::current(), path: pattern });
        assert_eq!(found.len(), 1);
        assert!(resolve(&Place { os: if Os::current() == Os::Mac { Os::Linux } else { Os::Mac }, path: pattern }).is_empty());
    }
}
