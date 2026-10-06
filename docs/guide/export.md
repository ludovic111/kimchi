# Export

Press ⌘E (Ctrl+E), or **Export** in the top bar, to open the export dialog. Every frame is drawn by
the same compositor as the preview, at full quality, and the sound comes from the same mixer.

## Formats

| Format | Codec | File | For |
| --- | --- | --- | --- |
| MP4 | H.264 | `.mp4` | Plays everywhere (the default) |
| HEVC | H.265 | `.mp4` | About half the size at the same quality |
| ProRes | ProRes 422 HQ | `.mov` | Further finishing in another editor |
| WebM | VP9 (AV1 with some GPU encoders) | `.webm` | The web |
| GIF | | `.gif` | Short loops, without sound; at most 720 pixels wide by default |
| Audio | WAV, AIFF, FLAC, MP3, AAC, Opus or Ogg | `.m4a` by default | Sound only |

## Options

| Option | Choices |
| --- | --- |
| Resolution | The project's size, 720p, 1080p or 4K. A preset sets the shorter side and keeps the project's shape. |
| Frame rate | The project's, 24, 25, 30 or 60. GIFs default to 15 and go up to 30. |
| Range | **Whole cut**; **Selected clips**, when clips are selected (from the earliest start to the latest end among them); or **Custom**, from and to a time. |
| Quality | Draft, **Standard** or High. |
| Encoder | **Auto**, GPU or CPU (not for GIF or sound only). |
| Captions | When the project has captions: In the picture, .srt file, Both or Off. See [Captions](captions.md#in-the-export). |
| Loudness | As mixed, −14 YouTube, −16 Podcast or −23 TV. |

**Loudness** sets the master's loudness target, which is part of the project (and an undo step),
so later exports keep it. See [Sound](sound.md#loudness).

**Quality** sets how much data the encoder uses and how long it spends finding the best encoding
(Draft is fastest, High slowest). With the processor's encoders (a lower quality factor keeps more
detail and makes a bigger file):

| | Draft | Standard | High |
| --- | --- | --- | --- |
| MP4 (H.264), quality factor | 28 | 22 | 18 |
| HEVC, quality factor | 30 | 26 | 22 |
| WebM (VP9), quality factor | 40 | 33 | 28 |
| AAC sound in MP4 and HEVC | 128 kbit/s | 192 kbit/s | 256 kbit/s |
| Opus sound in WebM | 96 kbit/s | 128 kbit/s | 160 kbit/s |

ProRes carries uncompressed 16-bit sound. Hardware encoders have their own quality scales, which
kimchi maps the three levels onto; those that take none get a bitrate in proportion to the picture's
size and frame rate. Quality doesn't change GIFs or sound-only exports, which have their own
settings.

### Hardware encoding

With **Auto**, kimchi encodes on the graphics hardware when the computer has a suitable encoder
(Apple VideoToolbox, NVIDIA NVENC, AMD AMF, Intel Quick Sync, VA-API, Windows Media Foundation) and
falls back to the processor if that encoder fails. The first time the dialog opens, kimchi checks
each hardware encoder with a short test encode (about a second); the note under **Encoder** then
says what will be used. **GPU** insists on hardware; **CPU** never uses it. `KIMCHI_HARDWARE=0`
turns hardware encoding off entirely.

## Sound only

The **Audio** format adds:

| Option | Choices |
| --- | --- |
| File type | WAV, AIFF, FLAC, MP3, **AAC**, Opus, Ogg (Vorbis) |
| Sample rate | **Project**, 44.1 kHz, 48 kHz or 96 kHz (Opus is always 48 kHz; MP3 at most 48 kHz) |
| Bit depth (WAV, AIFF, FLAC) | 16-bit, **24-bit**, or 32-bit float (WAV only) |
| Bitrate (MP3, AAC, Opus, Ogg) | 128, 192, **256** or 320 kbit/s |
| Stems | One file per track and bus, in a new folder |

16-bit WAV and AIFF files are dithered. Long WAVs are written as RF64 when they pass 4 GB.

**Stems** writes one file for each unmuted track with sound in the range, then one for each bus
something feeds, named after the track with a number in front (`01-Dialogue.m4a`, `02-Music.m4a`…,
in the chosen file type). Each stem is the track as the mix hears it: its clips, effects, fader and
pan. Turn on **Through the master's effects and limiter** to run each stem through the master as
well (and its loudness target).

## Exporting

Click **Export 1920×1080** (the button names the size; **Export audio** for sound only), or press
Enter. kimchi asks where to save the file (in the Movies or Videos folder, named after the project)
and starts. The dialog shows the progress and the encoder in use; **Hide** keeps the export going in
the background, and **Cancel export** stops it. A finished export offers to show the file in the
Finder (or Explorer, or the file manager). The file is written under a temporary name and renamed
when complete, so a cancelled or failed export never leaves a broken file.

Exports started by the agent or a script appear in the same dialog.

From a script:

```sh
kimchi-cli export.start --path ~/Movies/cut.mp4 --quality high --wait
kimchi-cli export.start --path ~/Movies/teaser.gif --format gif --from 10 --to 16 --width 640
```

See [AI_CONTROL.md](../AI_CONTROL.md#export) for every parameter.
