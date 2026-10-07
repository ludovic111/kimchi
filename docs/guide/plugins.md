# Plugins

Plugins add effects, generators and transitions to kimchi. Open **Plugins** from the ⋯ menu in the
top bar, the command palette (⌘K, "Plugins") or kimchi › Plugins… on macOS. It has four parts.

## Stock

What ships in kimchi, by kind, with a line on what each does: its colour effects (in the
inspector's Colour section), its transitions, its looks, ryolune's sound effects (in the mixer),
and four plugins made with kimchi's plugin SDK, which show how one is built:

| Plugin | Kind | What it does |
| --- | --- | --- |
| Halftone | effect | The picture as dots that grow with darkness, in one ink or in colour |
| Chromatic aberration | effect | Red and blue shifted apart from green, like a cheap lens |
| Gradient | generator | A linear or radial blend between two colours (put it on a solid) |
| Radial wipe | transition | The next picture sweeps in like a clock hand |

## Installed

Plugins found on this computer: **lsuite plugins** (the ones your agent builds, or bundles someone
gave you), **frei0r** filters (the free video filters Kdenlive and Shotcut use; install the
`frei0r-plugins` package, or `brew install frei0r` on a Mac), and the **sound plugins** ryolune's
engine found (CLAP, VST3, Audio Units, ryolune's own). Each row has the format's logo, the maker and
version, and a switch that is always in view: a plugin switched off stays installed, and clips that
use it are drawn without it (their settings are kept). lsuite plugins also have **Remove**.
**Rescan** looks again; files that didn't load are listed with the reason.

If a plugin crashes while drawing, kimchi switches it off, says so, and goes on: the picture is drawn
without it. Fix or update the plugin, then switch it on again.

## Formats

What kimchi loads, with each format's logo and the folders it looks in: lsuite plugins
(`~/.lsuite/plugins/kimchi`), frei0r (its usual folders, or `FREI0R_PATH`), LUTs (`.cube`, `.3dl`,
`.csp`, `.spi1d`, `.spi3d`, Hald CLUT pictures, imported as looks), and for sound ryolune's effects,
CLAP, VST3 and Audio Units (macOS). Nothing else is listed: these are the formats kimchi really runs.

## Build with your agent

Describe the plugin you want ("a VHS look with wobbly lines and colour bleed", "a transition where
the picture shatters into squares") and press **Build with your agent**. The Agent panel opens and
your agent follows kimchi's recipe: it reads the plugin guide, makes a Rust crate from the SDK's
template, writes the code, builds it until it compiles, installs it, puts it on the selected clip
and looks at a frame. You follow each step as a card. The new plugin appears in Installed and in
the inspector without a restart; ask for changes and it builds again, and the new version replaces
the old one at once.

Building needs Rust. Plugins says whether it is installed and, if not, gives the one command that
installs it (rustup, from rustup.rs); kimchi never installs anything by itself. Building also needs
the agent's **Plugins** permission: sending a request here turns it on (you can switch it off in
Settings › Agent). The sources stay in `~/.lsuite/plugins-src/kimchi/<name>/`.

## Using a plugin

Select a clip and open the inspector's **Plugins** section: **Add** lists the effect and generator
plugins. Each plugin on the clip has a switch (bypass), its settings (sliders, switches, choices)
and a remove button. Plugins run after the clip's colour effects, first to last, in the preview and
the export alike. A transition plugin goes between clips: `transition.set` with `plugin` (the
transition's own kind is drawn on computers without the plugin).

From scripts and agents: `plugin.list`, `plugin.info`, `clip.addPlugin`, `clip.setPlugin`,
`clip.setKeyframes` on `plugins.<slot>.<parameter>`, and the recipe commands `plugin.guide`,
`plugin.toolchain`, `plugin.new`, `plugin.writeSource`, `plugin.build`, `plugin.publishLocal`.
[PLUGINS.md](../PLUGINS.md) is the plugin author's reference.
