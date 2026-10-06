# Editing

This chapter covers bringing media in, placing it on the timeline, and cutting it: tracks,
clips, selection, trimming, splitting, speed and the inspector. Transitions and colour have
[their own chapter](transitions-and-colour.md); keyframes are in
[Motion graphics and 3D](motion.md#keyframes-on-any-clip).

## Media

The **Media** tab (⌘1) lists everything imported into the project or generated for it, newest
first. Search it by name, filter it (All, Video, Images, Audio, Generated), and switch between a
grid and a list.

**Importing.** Click **Import** (or press ⌘I) and pick files, or drop files anywhere
on the window. Dropping them on a track also places them there, one after another from the drop
point. Each file is checked with ffprobe and becomes a video, image or audio item; ryolune songs
(`.ryolune`) are accepted too (see [Sound](sound.md#ryolune-songs)). kimchi refers to imported
files where they are; it doesn't copy them into the project.

**Placing.** Drag an item onto a track to place it where you drop it; double-click it, or click the
**+** on its tile in the grid, to put it at the playhead. While dragging, the drop box turns red
over a track that can't take it (an audio file on a video track, or a locked track).

**Right-click an item** to insert it at the playhead, animate or edit an image with AI,
regenerate a generated item or make a variation of it, show the file in the file manager, or
**Remove from project…**. Removing an item also takes its clips off the timeline; kimchi says how
many first, and you can undo it. (Pressing Delete with an item selected and no clips selected asks the
same.) Selecting an item shows its details in the inspector.

## Titles

The **Text** tab (⌘3) adds titles at the playhead (T adds a plain one). Six presets give a
starting point: Title, Editorial, Lower third, Caption, Label and Shout. Below them, the font
list applies a font to the selected titles, or starts a new title in that font when none is
selected. Edit the words and style in the [inspector](#the-inspector).

A new title is 4 seconds long with 0.2-second fades, in Manrope Bold at 120 pixels, white,
centred, with a soft shadow.

**Solids** (a plain colour filling the frame) have no button yet: add one with the `clip.addSolid`
command, from the [Agent panel](agent.md), `kimchi-cli` or MCP. Their colour is edited in the
inspector.

## The timeline

### Tracks

There are two kinds of track: **video** tracks hold pictures (video, images, titles, solids and
motion clips) and **audio** tracks hold sound. A new project has "Video 1" and "Audio 1". New
video tracks go on top and new audio tracks at the bottom; the topmost video track is drawn in
front.

- **Add** a track with the toolbar buttons, the **+** beside "Tracks" in the ruler, or a track's
  menu.
- **Rename** a track by double-clicking its name.
- **Reorder** tracks by dragging their headers.
- **Right-click** a header to rename it, add a video track above or an audio track below, move
  it up or down, or delete it.

The header's toggles:

| Toggle | Effect |
| --- | --- |
| Eye (video tracks) | Hide the track's pictures in the preview and the export. |
| Speaker | Mute the track's sound. |
| **S** (tracks with sound) | Solo: only soloed tracks are heard. |
| **R** (audio tracks) | Arm the track for a [voice-over](sound.md#recording-a-voice-over). |
| Lock | Refuse changes to the track's clips: moving, trimming, splitting, deleting, pasting onto it. Locked lanes are hatched. |

Hidden and muted tracks draw their clips faded. A captions track is a video track that holds the
project's [captions](captions.md).

### Clips

A clip is a piece of media placed on a track, a title, a solid, a [motion clip](motion.md), or a
**placeholder** for a shot that is still [generating](generation.md). Images and solids are 5
seconds long when placed and titles 4; video and audio take the length of the file.

Badges on a clip show, when they apply: its name (a title's first line, a placeholder's prompt); its
kind; ✦ for generated media; a bot icon for clips an agent or script made or changed in the last
minute; a palette icon when it has colour effects; ◀ when it plays backwards; its speed (2×);
keyframe diamonds along its bottom edge; and, for motion clips, whether it is drawn **Live**,
**Rendered**, **Out of date** or **Rendering**.

### Selecting

- Click a clip to select it. Shift-click (or ⌘/Ctrl-click) adds it to the selection, or removes
  it.
- Drag across empty track space to select every clip the box touches.
- ⌘A selects every clip; Esc deselects.
- Clicking empty track space moves the playhead there.
- Double-clicking a title focuses its words in the inspector; double-clicking a motion clip opens
  it in [the Studio](motion.md#the-studio).

### Moving

Drag a clip to move it; every selected clip moves with it. A clip can move to another unlocked
track of the same kind. **A clip dropped over other clips overwrites them**: the parts it covers
are cut away.

Hold ⌥ (Alt) while dragging to drop copies and leave the originals in place.

⌥← and ⌥→ (Alt+arrows) nudge the selection by one frame; add Shift for ten frames.

**Snapping** is on by default (toggle it with N or the magnet button). A dragged clip, edge or
playhead sticks to time zero, the playhead, markers, and the starts and ends of other clips,
within 8 pixels on screen. With Settings › Audio › **Snap to beats** on, it also sticks to the
beats detected in music.

### Trimming

Drag either edge of a clip to trim it. A trim stops at the neighbouring clip and at the end of
the source media; images, titles and solids can be stretched as long as you like. Keyframes stay
where they are on the timeline, and fades shrink to fit.

**Q** trims the start of the clip under the playhead to the playhead, and **W** trims its end.
With clips selected, only those are trimmed; otherwise every clip under the playhead on unlocked
tracks is.

### Fades

Drag the round knob at either top corner of a clip to fade it in or out. The fade applies to both
its picture and its sound. The picture always fades evenly; the sound follows the clip's fade shape
(set in the inspector's Audio section), which the shaded wedge shows. The **Fade in** and **Fade
out** fields in the inspector's Timing section set the same lengths.

### Splitting

**S** (or ⌘B) splits at the playhead: the selected clips, or with nothing selected, every clip
under the playhead on unlocked tracks. The right half keeps no fade-in or transition and the left
half no fade-out.

### Deleting and closing gaps

Delete (⌫) removes the selection and leaves a gap. **⇧⌫** deletes and closes the gap: the later
clips on the same track move left by the deleted clip's length. Other tracks don't move.

With the toolbar's **Ripple delete** toggle on, a plain Delete closes the gap too. (Cut, ⌘X,
never does.)

Right-click a gap between two clips and choose **Close gap** to pull the later clips on that track
left.

### Copy, cut, paste, duplicate

- ⌘C copies the selected clips and ⌘X cuts them.
- ⌘V pastes at the playhead, each clip on the track it came from, and selects the result. The
  playhead moves to the end of the pasted clips, so pressing ⌘V again lays copies end to end.
  A paste lands whole or not at all: if any clip can't go where it should (a locked track, for
  example), nothing is pasted.
- Right-click empty track space and choose **Paste here** to paste at that point. Clips copied
  from one track land on the track you clicked.
- ⌘D duplicates the selected clips to the end of their tracks.

### Right-clicking

**A clip:** split and trim at the playhead; copy, cut, duplicate; add or remove a transition; play
backwards; **Freeze frame here**; for clips with sound, add an effect, measure or normalise
loudness, and detect beats or cut on them; for pictures, the [generation
actions](generation.md#generation-in-the-edit) (animate this frame, extend, restyle); **Bridge with
AI** when two clips on video tracks are selected; for generated media, regenerate or make a
variation; for motion clips, open in the Studio and render; delete and ripple delete.

**Empty track space:** generate a video or an image sized to the gap (or, past the track's last
clip, starting where that clip ends), paste here, add text here, add a marker, move the playhead
here, close the gap.

### Markers

**M** and the toolbar's pin button add a marker at the playhead; **Add marker here** (right-click
empty track space) adds one where you clicked.
Markers appear in the ruler; click one to jump to it, right-click it to remove it. ↑ and ↓ jump
between cuts and markers. Give a marker a label with `timeline.addMarker --label …`; the window
doesn't edit labels.

### Moving around

| To | Do |
| --- | --- |
| Move the playhead | Click or drag in the ruler, or drag the playhead line |
| Play, shuttle | Space; J, K and L (each press of J or L plays faster) |
| Step | ← → one frame, ⇧← ⇧→ one second, ↑ ↓ to the previous or next cut or marker, Home and End to the start and end |
| Zoom | =, −, the slider, or ⌘ (Ctrl) + scroll, or pinch; ⇧Z or **Fit** shows the whole cut |
| Scroll | The scroll wheel; Shift + scroll sideways |

While playing, the timeline follows the playhead. **Loop** (⌘L) repeats playback.

## The inspector

The inspector edits the selection. Most sections fold away; click a section's title to fold it.

**One clip.** The header has the clip's name (click it to rename the clip) and its kind. Then,
depending on the clip:

| Section | For | What's in it |
| --- | --- | --- |
| Provenance | Generated media | The prompt, provider, model, seed, time taken, cost and inputs; **Regenerate** and **Variation**. |
| With AI | Pictures | Animate this frame, Extend shot, Restyle frame. |
| Text | Titles | The words, font, colour, size (8–800), weight (100–900), letter spacing ("Track"), line height ("Line"), alignment, italic, shadow, and a box behind the words. |
| Colour | Solids | The fill colour. |
| Motion clip, Scene | Motion clips | Open in the Studio, render controls, template values and a quick scene editor. See [Motion graphics and 3D](motion.md). |
| Transform | Clips on video tracks | X and Y (pixels from the centre, positive Y is down), Scale (1–1000%), Rotate (−360° to 360°), Opacity, and for media how it fits the frame: **Fit** (whole picture, letterboxed), **Fill** (cropped to fill) or **Stretch**. A reset button puts back position, scale, rotation and opacity. |
| Animation | Clips on video tracks | Keyframes, easing and presets. See [Keyframes on any clip](motion.md#keyframes-on-any-clip). |
| Colour | Clips on video tracks | Looks, corrections, chroma key and LUT. See [Colour](transitions-and-colour.md#colour). |
| Audio | Clips with sound | Gain, pan, fade shape, channels, pitch, effects, loudness and beats. See [Sound](sound.md#a-clips-sound). |
| Timing | Every clip | Start, end and length; speed; fades; play backwards; freeze frame. |
| Transition in | Clips on video tracks, and clips with sound | How the clip comes in (**Crossfade in** on audio tracks). See [Transitions](transitions-and-colour.md#transitions). |
| Source | Media | The file's name, picture size, length, frame rate, codecs and file size, and a button to show it in the file manager. |

Number fields are scrubs: drag across one to change it (Shift for bigger steps, Alt for finer
ones), or click to type a value. The gain, pan and colour sliders reset when double-clicked.

**On the picture.** Click a picture in the preview to select its clip (Shift-click adds); drag it to
move it, or drag its handles to scale it. Moves snap to the canvas's centre lines.

**Several clips.** The count, total length and span, Duplicate and Delete; with exactly two
clips on video tracks, **Bridge these two shots** (see [Generation](generation.md)).

**A media item.** Its poster, provenance, details, **Insert** and **Reveal**; for images, Animate
with AI and Edit with AI.

**Nothing selected.** The project's settings: **Canvas** size (presets 1080p, 4K, Vertical, Square
and 4:5, or any width and height from 16 to 7680), **Frame rate** (24, 25, 30, 50 or 60) and
**Background** colour. Other frame rates and the audio sample rate are set with
`project.setSettings`.

## Speed, reverse and freeze frames

**Speed** (Timing section, 0.1× to 16×, with 0.25×, 0.5×, 1×, 2× and 4× buttons) keeps the
clip's in and out points in the source, so the clip gets shorter when faster and longer when
slower. A clip that grows pushes the later clips on its track to the right. Sound keeps its pitch
unless you turn off **Speed keeps the pitch** in the Audio section; then it plays like tape.

**Play backwards** (Timing section or the clip's menu) plays the same part of the source in
reverse, sound included. It applies to video and audio clips.

**Freeze frame at the playhead** splits the clip at the playhead and holds that frame for
2 seconds, pushing the rest of the track later. Except for images, which hold their own file, the
held frame is saved as a new image in the project's media.

## Undo and saving

Every change is one undo step: ⌘Z undoes, ⇧⌘Z (or ⌘Y) redoes, and a notice says what was undone.
Drags, sliders and nudges in quick succession fold into one step. The history is shared with the
agent and with scripts, so ⌘Z also undoes what they did; the [Agent panel](agent.md#changes)'s
**Changes** tab shows the history. It holds up to 300 steps, lasts while the project is open, and
starts afresh when the project is reopened.

kimchi saves every change as you make it; there is nothing to save by hand. Each project is a
folder in kimchi's library (see [Configuration](../CONFIGURATION.md#files-and-folders)).
