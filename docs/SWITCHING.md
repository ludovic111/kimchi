# Switching to kimchi

You can bring a cut from another editor into kimchi and keep working on it, and send a kimchi cut
back. In the window: **Home › Open from another editor** (or the editor's **⋯** menu), and
**⋯ › Export project for <app>…**. From a terminal or an agent: `project.importFrom`,
`project.exportTo`, `project.formats` and `media.relink` (see [COMMANDS.md](COMMANDS.md)).

After each trip kimchi shows what came through, what changed on the way and what was left out.
If media files aren't where the project says, choose **Find missing files…** and pick the folder
they are in: kimchi finds them by name, in that folder and the ones inside it.

## What kimchi reads and writes

| Format | Open | Write | Used by |
|---|---|---|---|
| OpenTimelineIO (`.otio`, `.otioz`, `.otiod`) | yes | yes | DaVinci Resolve, Kdenlive, Nuke Studio, Hiero, Avid |
| FCPXML (`.fcpxml`, `.fcpxmld`) | yes | yes (1.10) | Final Cut Pro, DaVinci Resolve |
| Final Cut Pro 7 XML (`.xml`) | yes | yes | Premiere Pro, DaVinci Resolve, VEGAS Pro |
| CMX 3600 EDL (`.edl`) | yes | yes | Avid Media Composer, every editor |

Not yet: Kdenlive (`.kdenlive`) and Shotcut (`.mlt`) projects, OpenShot (`.osp`), Premiere Pro
projects (`.prproj`) and CapCut drafts. Use the routes below instead.

## App by app

**Premiere Pro.** Select the sequence and choose File › Export › Final Cut Pro XML…, then open the
`.xml` in kimchi. Back: Export project for Premiere Pro, then File › Import in Premiere.
Survives: tracks, cuts, in points, speed, dissolves, scale, position, rotation, opacity, volume
and pan with their keyframes, colour mattes, markers. Doesn't: Premiere's own effects (Lumetri,
blurs: listed as left out), titles made with the Essential Graphics panel.

**Final Cut Pro.** Select the project and choose File › Export XML…, then open the `.fcpxml`.
Back: Export project for Final Cut Pro, then File › Import › XML…. Survives: the primary storyline
and connected clips (on tracks above, sound below), gaps, transitions, transform, opacity,
volume, speed (timeMap), titles with their font, size and colour, markers and chapter markers,
roles as track names. Doesn't: crops, blend modes, effects; compound clips are flattened; an
audition comes as its chosen clip. kimchi writes all transitions as cross dissolves.

**DaVinci Resolve.** File › Export › Timeline… as OpenTimelineIO (best) or FCPXML. Back: Export
project for DaVinci Resolve (OpenTimelineIO), then File › Import › Timeline…. kimchi keeps
everything of its own in the file's kimchi metadata, so a cut that goes to Resolve and comes back
keeps its titles, effects and sound settings.

**Avid Media Composer.** Save a CMX 3600 EDL with the List Tool and open it. Back: Export project
for Avid (EDL). An EDL holds one picture track and two sound tracks, cuts, dissolves, wipes and
speeds: nothing else. Media is found by its file name.

**VEGAS Pro.** File › Export › Final Cut Pro 7 / DaVinci Resolve (*.xml), and back the same way.

**Kdenlive.** File › Export › OpenTimelineIO, and back with Export project for Kdenlive.

**Shotcut, OpenShot, CapCut, iMovie.** kimchi can't read their projects yet: export the video and
import it. (iMovie: File › Send Movie to Final Cut Pro, then FCPXML.)

## What changes on the way

- Transitions are centred on the cut in kimchi; ones that start or end at the cut keep the same
  frames on screen. Names kimchi doesn't have become the closest kimchi transition (said in the
  report).
- A clip's sound stays with its picture when the other app kept it lined up on an audio track;
  J and L cuts stay as separate sound clips.
- Titles, colour clips and motion clips are drawn by kimchi. When writing, kimchi renders them to
  ProRes files beside the project (over black for now) so the other app shows them.
- Times go through exact frame counts (29.97 and 23.976 included), so cuts land on the same frames.
