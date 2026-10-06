# Keyboard shortcuts

Press **?** in the window (or ⌘/ on macOS, Ctrl+/ elsewhere) for the same list (without Quit), drawn
from the table the window binds its keys from (`actions::SHORTCUTS`). Menus, tooltips and the
command palette show the same keys.

On Linux and Windows, ⌘ is Ctrl and ⌥ is Alt. Where a row lists several keys, any of them works.

## Where a shortcut works

- **Single keys and editing keys** (Space, S, ⌘Z, ⌘C…) work in the editor, but not while you
  are typing in a text field or a dialog is open. Esc is the exception: it still closes dialogs.
- **Panel and project keys with a modifier** (⌘K, ⌘1, ⌘J, ⌘E…) work anywhere in the window, even
  while typing.
- **Studio keys** apply while the Studio is open and take precedence over the editor's keys
  there. They pause while a Studio tool or menu is in use.
- **Quit** (⌘Q) always works.

In a one-line text field, Enter confirms and Esc cancels. In the prompt fields (Generate, the
Agent) and a title's words, Enter starts a new line and ⌘Enter (Ctrl+Enter) sends; the prompt on
home sends with Enter. In the command palette, ↑ and ↓ (or Ctrl+P and Ctrl+N) move.

## Playback

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Play / pause | Space | Space |
| Play backwards, faster each press | J | J |
| Stop | K | K |
| Play forwards, faster each press | L | L |
| Loop playback | ⌘L | Ctrl+L |
| Previous / next frame | ← / → | ← / → |
| Back / forward one second | ⇧← / ⇧→ | Shift+← / Shift+→ |
| Previous / next cut or marker | ↑ / ↓ | ↑ / ↓ |
| Go to start / end | Home / End | Home / End |

## Editing

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Undo | ⌘Z | Ctrl+Z |
| Redo | ⇧⌘Z, ⌘Y | Ctrl+Shift+Z, Ctrl+Y |
| Copy | ⌘C | Ctrl+C |
| Cut | ⌘X | Ctrl+X |
| Paste at the playhead | ⌘V | Ctrl+V |
| Duplicate | ⌘D | Ctrl+D |
| Split at the playhead | S, ⌘B | S, Ctrl+B |
| Trim start to the playhead | Q | Q |
| Trim end to the playhead | W | W |
| Nudge one frame left / right | ⌥← / ⌥→ | Alt+← / Alt+→ |
| Nudge ten frames left / right | ⌥⇧← / ⌥⇧→ | Alt+Shift+← / Alt+Shift+→ |
| Delete | ⌫, ⌦ | Backspace, Delete |
| Delete and close the gap | ⇧⌫, ⇧⌦ | Shift+Backspace, Shift+Delete |
| Select all | ⌘A | Ctrl+A |
| Deselect | Esc, ⇧⌘A | Esc, Ctrl+Shift+A |

## Timeline

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Add a title | T | T |
| Add a marker | M | M |
| Snapping on / off | N | N |
| Zoom in | =, +, ⌘= | =, +, Ctrl+= |
| Zoom out | −, ⌘− | −, Ctrl+− |
| Zoom to fit | ⇧Z, \\, ⌘0 | Shift+Z, \\, Ctrl+0 |
| Open the selected motion clip in the Studio | ⇧⌘O | Ctrl+Shift+O |

## Panels

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Command palette | ⌘K | Ctrl+K |
| Generate (focus the prompt) | ⌘G | Ctrl+G |
| Media / Generate / Text / Motion / Captions tab | ⌘1 … ⌘5 | Ctrl+1 … Ctrl+5 |
| Agent | ⌘J | Ctrl+J |
| Show / hide the left panel | ⌥⌘B | Ctrl+Alt+B |
| Show / hide the inspector | ⌥⌘I | Ctrl+Alt+I |
| Keyboard shortcuts | ?, ⌘/ | ?, Ctrl+/ |

## Project

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Import media | ⌘I | Ctrl+I |
| Export | ⌘E | Ctrl+E |
| Save (kimchi already saves every change) | ⌘S | Ctrl+S |
| New project | ⌘N | Ctrl+N |
| All projects (close this one) | ⌘W | Ctrl+W |
| Settings | ⌘, | Ctrl+, |
| Quit | ⌘Q | Ctrl+Q |

## Audio

The selected track is the selected mixer strip, or the track of the first selected clip.


| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Show / hide the mixer | X | X |
| Mute the selected track | ⌥M | Alt+M |
| Solo the selected track | ⌥S | Alt+S |
| Arm the selected track for recording | ⌥A | Alt+A |
| Record a voice-over / stop | ⇧R | Shift+R |
| Add an effect | ⇧⌘E | Ctrl+Shift+E |

## The Studio

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Back to the edit | Esc | Esc |
| Play / pause the clip | Space | Space |
| Add | ⇧A | Shift+A |
| Move (2D: pen) | G | G |
| Rotate | R | R |
| Scale | S | S |
| Delete | X, ⌦, ⌫ | X, Delete, Backspace |
| Duplicate | ⇧D, ⌘D | Shift+D, Ctrl+D |
| Select all / none | A, ⌘A | A, Ctrl+A |
| Box select | B | B |
| Hide selected | H | H |
| Show everything | ⌥H | Alt+H |
| Keyframe here (edit mode: inset) | I | I |
| Front view (edit mode: vertices) | 1 | 1 |
| Edges (edit mode) | 2 | 2 |
| Right view (edit mode: faces) | 3 | 3 |
| Top view | 7 | 7 |
| Through the camera | 0 | 0 |
| Perspective / orthographic | 5 | 5 |
| Frame the selection | . | . |
| Frame (edit mode: fill) | F | F |
| Frame everything | Home | Home |
| Fit the canvas (2D) | ⇧Z | Shift+Z |
| Zoom in / out | =, + / − | =, + / − |
| Canvas at 100% (2D) | / | / |
| Fly through the scene (WASD, QE, mouse to look) | ⇧\`, ~ | Shift+\`, ~ |
| Align the active camera to the view | ⌥⌘0 | Ctrl+Alt+0 |
| Select tool | V | V |
| Next tool | W | W |
| Pen (2D) | P | P |
| Shape tools (2D) | Q | Q |
| Text tool (2D) | T | T |
| Anchor point tool (2D) | Y | Y |
| Dope sheet / graph editor | ⇧⌘G | Ctrl+Shift+G |

### Modelling (3D edit mode)

| Action | macOS | Linux / Windows |
| --- | --- | --- |
| Edit mode on / off | Tab | Tab |
| Extrude | E | E |
| Bevel | ⌘B | Ctrl+B |
| Loop cut | ⌘R | Ctrl+R |
| Merge | M | M |
| Flip normals | ⌥N | Alt+N |
| Recalculate normals | ⇧N | Shift+N |

## Mouse

| Action | How |
| --- | --- |
| Select several clips | Drag on empty track space |
| Add to the selection | Shift-click |
| Copy a clip | Hold ⌥ (Alt) while dragging it |
| Trim | Drag a clip's edge |
| Fade in / out | Drag the round knob on top of a clip |
| Move the playhead | Click or drag the ruler |
| Zoom the timeline | Pinch, or ⌘ (Ctrl) + scroll |
| Scroll sideways | Shift + scroll |
| Rename a track | Double-click its name |
| Reset a panel's size | Double-click its divider |
| More actions | Right-click anything |
| Studio: orbit the 3D view | Middle-drag, or ⌥-drag (Alt-drag) |
| Studio: pan the view | Shift + middle-drag, or Space-drag |
| Studio: zoom | Mouse wheel, pinch, or ⌘ (Ctrl) + scroll; on a trackpad, two-finger scrolling orbits (3D) or pans (2D) |
| Studio: look around and fly (3D) | Hold the right button and drag |
| Open a motion clip in the Studio | Double-click it |

## Actions without a key

These are in the command palette (and, except Restart kimchi, in the macOS menus): About, Check for
updates, What's new, the Generations list, Toggle light / dark, User guide,
Logs and crash reports, Report a problem, Restart kimchi and Support kimchi. Scripts and agents can
run any of them by name with `ui.action` (see [Controlling kimchi from AI and
scripts](../AI_CONTROL.md#the-windows-shortcuts-by-name)).
