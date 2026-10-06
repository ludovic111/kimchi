# Captions

Captions are titles on a **captions track**, a video track at the top of the timeline. Because
they are ordinary titles, you can move, trim, split and restyle them like any other clip. The
**Captions** tab (⌘5) gathers what is specific to them.

## Transcribing the cut

**Transcribe the cut** listens to everything you hear on the timeline (unmuted tracks) and puts
the words on the captions track. It runs on your computer with OpenAI's Whisper model; nothing
is sent anywhere.

1. Choose a model. The first use downloads it once from Hugging Face:

   | Model | Download | Use |
   | --- | --- | --- |
   | Tiny | 151 MB | Fastest; fine for clear speech |
   | Base (default) | 290 MB | A good balance of speed and accuracy |
   | Small | 967 MB | Most accurate; several times slower |

2. Leave the **Detect the language** field empty to detect it from the first 30 seconds, or type a
   language code (`en`, `fr`, `es`, `de`, `ja`…).
3. Click **Transcribe the cut** (**Transcribe again** once there are captions; it is unavailable
   when nothing on the timeline makes a sound). The panel shows the download, then "Mixing the
   sound…", then "Listening…" with a percentage. **Cancel** stops it.

Transcribing again replaces the captions already there. Captions are split into at most two
lines of 42 characters, and a pause of a second or more starts a new caption. If no speech is
heard, kimchi says so and adds nothing. Only one transcription runs at a time.

Models are kept in kimchi's data folder, under `models/whisper-tiny`, `whisper-base` and
`whisper-small`; delete one to free the space (it downloads again when needed).

Whisper runs on the processor. On a four-core laptop, the base model takes about 5 seconds for
11 seconds of speech.

## Subtitle files

- **Import…** reads an `.srt` or `.vtt` file (UTF-8, UTF-16, or Windows-1252) and adds its
  captions to the track, replacing any captions they overlap. Styling tags in the file are dropped.
- **Export…** saves the captions as `.srt`, or as WebVTT if you name the file `.vtt`.
- **Add a caption at the playhead** adds a 2.5-second caption reading "New caption".

## Editing and styling

The panel lists every caption. Click one to jump to it; double-click it to edit its words in the
inspector. **Clear** removes them all.

**Look of every caption** changes all captions at once: their **Size**, their **Height** in the
frame, and a **Box behind the words**. To change one caption's font, colour or weight, select it
and use the inspector; to change them all, use `captions.setStyle`.

New captions are white Manrope SemiBold, sized at 4.8% of the frame's height, on a translucent
black box, placed in the lower part of the frame. When captions already exist, new ones copy the
first one's style.

## In the export

When the project has captions, the export dialog has a **Captions** row:

| Choice | Result |
| --- | --- |
| In the picture (default) | Burned into the video |
| .srt file | An `.srt` beside the video (`cut.srt` next to `cut.mp4`), and a clean picture |
| Both | Burned in, and the `.srt` |
| Off | Neither |

The `.srt` is written when the export starts, replacing a file of the same name. Sound-only
exports don't include captions.
