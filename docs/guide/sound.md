# Sound

All sound in kimchi comes from its own mixer: what you hear while playing and scrubbing, the
export, and what transcription listens to. ffmpeg only decodes the files and encodes the finished
mix, so what you hear is what you export.

Levels are in decibels: 0 dB leaves a sound as it was, and the bottom of every fader and gain
slider is silence. Pan runs from left (−1) through centre to right (1).

## How sound flows

1. **Each clip**: channels → volume (and its keyframes) → fades → the clip's effects → pan.
2. **Each track** adds up its clips → the track's effects → ducking → fader and pan → the master
   or a bus. Sends copy the track's sound to buses as well.
3. **Each bus**: effects → fader and pan → the master.
4. **The master**: effects → fader → the loudness target (exports only) → a true-peak limiter.

Effects that add delay are compensated, and effect tails (a reverb's echo) ring on after a clip
ends.

## A clip's sound

Select a clip with sound; the inspector's **Audio** section has:

| Control | What it does |
| --- | --- |
| Gain | −36 to +12 dB; the bottom of the slider mutes. Keyframable (◆). |
| Pan | Left to right. Keyframable (◆). |
| Fade shape | Linear, Equal (equal power), Exp (exponential) or S (S-curve), for this clip's fades. |
| Channels | Stereo, Mono, L or R (one side on both), or Swap. |
| Pitch | −24 to +24 semitones. |
| Speed keeps the pitch | On by default; off, a faster clip sounds higher, like tape. |
| Mute this clip's sound | Silences the clip without touching its track. |
| Effects | The clip's own effect chain, up to 8. |
| Loudness | **Measure** reads its loudness and peak; **Normalize to −16 LUFS** sets its gain to the target in Settings › Audio. |
| Beats | **Detect beats** finds its tempo and beats (see [Beats](#beats)). |

The fade lengths are in the Timing section, or the knobs on the clip (see
[Editing](editing.md#fades)).

**On the timeline**, clips on audio tracks (and a selected video clip with sound) show a volume
line. Drag it up or down to set the gain (Shift for finer steps), double-click it to add a keyframe,
drag keyframe points in time and level, and right-click a point to remove it.

## The mixer

Press **X** (or the mixer button in the timeline's toolbar, or Audio › Mixer on macOS) to show the
mixer. Beside that button are a level meter for the whole mix and **Record a voice-over**; track
headers have small meters too.
It takes the tracks' place, or sits beside them; the layout button next to the mixer button
switches between the two.

There is a strip for each track with sound, then one for each bus, then a **+** to add a bus, and
the master on the right. Short timelines show less on each strip: below about 330 pixels a strip's
effects fold into one chip and its sends are hidden; below about 236 the output button goes too.

A strip, from top to bottom:

- **Name.** Click it to show the strip's settings in the inspector; right-click for its menu.
- **Effects.** Click one to open its panel, drag to reorder, click its dot to bypass it.
  **+ Effect** adds one (up to 8).
- **Sends** (tracks). **Send** adds one to a bus, at −12 dB; drag a send's bar to set its level and
  right-click it to remove it. **New bus with a reverb** makes a "Reverb" bus and sends to it.
  Sends are after the fader; `audio.setSend --preFader true` takes one before it.
- **Pan.** Drag up or down; double-click to centre.
- **Fader and meter.** Drag the fader (Shift for finer steps), double-click it for 0 dB, or click
  the level below it to type one (`-6`, `+3`, `off`). The meter shows peak and average level, from
  −60 to +6 dB, and holds its peak for a moment. The clip light turns on when the sound went over
  0 dB; click it to clear it.
- **M**, **S** and **R**: mute, solo, and (audio tracks) arm for a voice-over.
- **Output** (tracks): the master or a bus, or **New bus…**.

⌥M and ⌥S mute and solo the selected track (its strip, or the track of the selected clip).
**Solo** works as on a mixing desk: soloing a track keeps the buses it feeds, and soloing a bus
keeps the tracks that feed it.

**Strip menu** (right-click the name): reset the fader to 0 dB (or, once it is automated, remove the
fader's automation), automate the fader at the playhead,
add an effect; for tracks, copy their effects to every track, duck under the other tracks, and
measure loudness; for buses, remove the bus (its tracks go back to the master); for the master,
measure the mix.

Meters move only while playing.

### The master

The master strip shows loudness while playing: **M** momentary (400 ms), **S** short-term (3 s)
and **I** integrated over the last play, in LUFS. **Limit** switches the true-peak limiter (on by
default, at −1 dBTP; change the ceiling with `audio.setMaster --ceilingDb`). The **Target** button
sets the loudness the export is brought to: As mixed, YouTube and streaming (−14 LUFS), Podcast
(−16) or Broadcast (−23).

The target applies to exports only; playback in kimchi isn't adjusted to it.

### Automation

Fader, pan and effect parameters on tracks, buses and the master can change over time. Choose
**Automate the fader here** on a strip, or click ◆ beside an effect parameter, to add a keyframe at
the playhead. From then on, moving that control writes a keyframe at the playhead. Right-click an
effect parameter's ◆, or use the strip menu for the fader, to remove the automation. Automated
controls are drawn in the accent colour. Clip effects can't be automated; animate the clip's gain
and pan instead.

### Ducking

Ducking lowers a track (usually music) while other tracks are sounding (usually voices). Turn on
**Duck under the other tracks** in a track's strip menu, or click the strip's name and use the
inspector's **Ducking** section (**Duck under other tracks**): **Down by** sets how far (default −12
dB). By default a track ducks under every other track with sound that isn't ducked itself;
`audio.setTrack --duck` chooses which tracks, the threshold (−40 dB), and how fast it goes down
(0.15 s) and comes back (0.6 s).

**Duck the music under the dialogue**, in the same inspector section, sets this up in one step:
kimchi finds the music track by its name ("music", "song", "score", "bgm"…) or by its content (a
ryolune song, or detected beats).

## Effects

Choose **Add an effect** (⇧⌘E, the strip's **+ Effect**, or the clip's menu) to open the effect
browser. Type to search by name, maker, category or format; Enter adds the first match. ⇧⌘E adds
to the selected clip if it has sound, otherwise to the selected track, otherwise to the master.

**Built in** are ryolune's 23 stock effects (the compressor is listed as "ryolune Comp"):

| Category | Effects |
| --- | --- |
| Dynamics | Comp, Gate, Limiter, Transient, De-Esser, Pump |
| EQ & filter | Channel EQ, Filter, Auto Filter |
| Distortion | Tape Sat, Overdrive, Bitcrusher, Lo-Fi |
| Modulation | Chorus, Phaser, Tremolo, Flanger, Auto Pan |
| Pitch | Pitch Shift |
| Space & time | Space (reverb), Echo |
| Utility | Stereo Width, Utility |

**Plugins** installed on the computer appear too: CLAP and VST3 everywhere, Audio Units on macOS,
and ryolune's own plugin format (in ryolune's `plugins` folder, or `RYOLUNE_PLUGIN_PATH`). kimchi looks
in the standard folders:

| Format | macOS | Windows | Linux |
| --- | --- | --- | --- |
| CLAP | `~/Library/Audio/Plug-Ins/CLAP`, `/Library/Audio/Plug-Ins/CLAP` | `%COMMONPROGRAMFILES%\CLAP`, `%LOCALAPPDATA%\Programs\Common\CLAP` | `~/.clap`, `/usr/lib/clap`, `/usr/local/lib/clap` |
| VST3 | the same, with `VST3` | the same, with `VST3` | `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3` |

in the folders named by `CLAP_PATH` and `VST3_PATH`, and in the extra folders set in ryolune's own
Settings › Plugins. Each CLAP, VST3 and ryolune plugin is checked in a separate process, so one that
crashes can't take kimchi down. The list of plugins found is shared with ryolune. **Rescan plugins**
(in the browser or Settings › Audio) looks again after you install one.

> In 0.8.0, the extra folders listed under Settings › Audio › Plugins are saved but not yet
> scanned. For plugins outside the standard folders, use `CLAP_PATH`, `VST3_PATH` or ryolune's
> plugin settings.

**The effect panel** opens when you add or click an effect. Drag a knob (Shift for finer steps),
double-click it for its default, or click a value to type one (`-6 dB`, `2.5k`). Stock effects have
**Presets**. The power button bypasses the effect; reset returns every parameter to its default.
kimchi shows a plugin's parameters as knobs and switches; it doesn't open the plugin's own window.

A plugin that can't be loaded (missing on this computer, for example) is skipped and reported,
and its settings stay in the project.

## Loudness

kimchi measures loudness to EBU R128: integrated loudness in LUFS, true peak in dBTP and loudness
range in LU. **Measure loudness** works on a clip (on its own, with its effects), a track or the
whole mix (strip and clip menus, or the inspector's **Measure**), and shows the result.

There are two ways to reach a target loudness:

- **Normalize** a clip (inspector or clip menu) sets its gain so it reaches the target in
  Settings › Audio (−16 LUFS by default; YouTube −14, Broadcast −23). Gain is raised by at most
  +12 dB; silent clips are left alone.
- **Set the master's target** (the master's Target button, or the export dialog's Loudness row).
  At export, kimchi measures the finished mix, brings it to the target, then limits it.

## Beats

**Detect beats** (inspector or clip menu) finds a music clip's tempo, beats and downbeats; it
needs at least 4 seconds of music with a steady pulse. ryolune songs bring their own tempo map.
The beats then show as ticks along the bottom of the clip, taller on downbeats.

- **Snap to beats** in Settings › Audio makes clips, edges and the playhead stick to them.
- **Cut the picture on the bars** (clip menu) splits the clips on the top video track every 4 beats
  of the music (every bar in 4/4). `audio.beatCut` chooses another rhythm (`every` 1 for every
  beat), an offset, the track, or adds markers instead of cutting.

## Recording a voice-over

1. Choose the microphone in Settings › Audio › **Input**.
2. Optionally arm an audio track: its **R** button, or ⌥A with its strip or one of its clips
   selected. The take goes on the armed track (the first, if several are), or else on a new audio
   track.
3. Put the playhead where the take should start and press **⇧R** (or **Record a voice-over** in the
   timeline's toolbar, or Audio › Record Voice-Over on macOS).
4. After the count-in (3 seconds by default; Settings › Audio › Voice-over count-in), playback
   starts and kimchi records. Press ⇧R again to stop.

The take is placed at the point where you started, as one undo step. Takes are mono 32-bit float
WAV files at the project's sample rate, saved in kimchi's `recordings/` folder. Undoing removes the
clip (and the track, if it was new) but keeps the recording in the project's media.

kimchi doesn't play the microphone back to you while recording; use your audio interface's
monitoring. If the take can't be placed, the file is kept and kimchi says where.

## ryolune songs

[ryolune](https://lsuite.xyz/ryolune) is lsuite's music app. kimchi plays its songs with ryolune's
own engine, built in, so ryolune doesn't need to be installed to use them.

- **Import a song** (`.ryolune`) like any other file. kimchi renders it to audio and uses the
  song's tempo map for its beats. `audio.importSong --as stems` imports one track per ryolune
  track instead.
- **Keep songs up to date** (Settings › Audio, on by default): when you come back to kimchi after
  saving the song in ryolune, it is rendered again. The inspector shows whether a song changed;
  its refresh button renders it again by hand.
- **Open in ryolune** (inspector or clip menu) opens the song in ryolune, starting it if needed.

**Sending the cut to ryolune** to score it: the palette's **Send the cut to ryolune to score**
(`handoff.toRyolune`) renders the cut's sound with its length and markers and, if ryolune is
running, opens it there. With `--as session`, it sends an editable multitrack session instead:
one ryolune track per kimchi track, with fades, gain, effects, faders, automation and buses kept.
`handoff.fromRyolune` brings ryolune's music back onto an audio track. See
[AI_CONTROL.md](../AI_CONTROL.md#hand-offs-with-ryolune).

## Devices and playback

Settings › Audio › **Output** chooses where the preview plays, and **Input** the microphone; both
default to the system's. If a chosen device disappears, kimchi uses the system default.

The preview mixes at 48 kHz in real time, about a tenth of a second ahead, so changes to faders,
effects and clips are heard almost at once without stopping. A file that isn't decoded yet plays
silence rather than holding playback up.

**Hear while scrubbing** (Settings › Audio, on by default) plays a short snippet of the mix
whenever the playhead moves while stopped.

## Exporting sound

See [Export](export.md#sound-only): WAV, AIFF, FLAC, MP3, AAC, Opus and Ogg, sample rate, bit depth
or bitrate, and stems (one file per track and bus).
