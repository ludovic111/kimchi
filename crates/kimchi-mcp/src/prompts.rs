//! MCP prompts: ready starts for common jobs, written against the registry's tools.

use serde_json::Value;

pub struct Prompt {
    pub name: &'static str,
    pub description: &'static str,
    /// name, description, required
    pub arguments: &'static [(&'static str, &'static str, bool)],
    pub render: fn(&Value) -> String,
}

/// A string argument, or `default` when it is missing or blank.
pub fn arg<'a>(arguments: &'a Value, key: &str, default: &'a str) -> &'a str {
    arguments.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default)
}

pub const PROMPTS: [Prompt; 6] = [
    Prompt {
        name: "rough-cut",
        description: "Assemble media into a first cut: import, order, trim to a target length, add a title card and markers.",
        arguments: &[
            ("media", "Files or a folder to import (absolute paths, one per line or comma separated). Omit to use the project's media.", false),
            ("length", "Target length in seconds (default: about 60)", false),
            ("style", "Pacing and mood, e.g. \"fast, punchy\" or \"slow, contemplative\"", false),
        ],
        render: |a| {
            let media = match arg(a, "media", "") {
                "" => "Use the media already in the project (media_list).".to_string(),
                m => format!("Import these with media_import (place=false first, so you can choose the order): {m}."),
            };
            format!(
                "Make a rough cut of about {length} seconds, {style}. Start with project_overview; if no project is open, project_create one. {media} \
                 Look at each item's length and, for generated media, its prompt. Put the shots on the timeline in a sensible order with clip_insertMedia \
                 (start = where the previous one ends), then shape the pace with clip_trim and clip_split; delete what doesn't serve the cut with clip_delete ripple=true. \
                 Keep music or ambience on an audio track under the pictures. Add a marker (timeline_addMarker) at each section change. \
                 Use project_batch for groups of edits so each pass is one undo step. Finish with project_overview, check problems is empty, and describe the cut shot by shot.",
                length = arg(a, "length", "60"),
                style = arg(a, "style", "with a natural rhythm"),
            )
        },
    },
    Prompt {
        name: "title-and-captions",
        description: "Add a title card and timed captions as text clips, styled to read well over the picture.",
        arguments: &[
            ("title", "The title to show at the start", true),
            ("captions", "Caption lines, one per line, optionally prefixed with a time in seconds (\"12.5 Hello there\")", false),
            ("style", "Look of the text, e.g. \"bold white, bottom third\" (default: clean white sans, centred low)", false),
        ],
        render: |a| {
            let captions = match arg(a, "captions", "") {
                "" => "No captions were given: if the project has spoken audio, ask the person for the lines rather than inventing them.".to_string(),
                c => format!("Then add these captions, one text clip each, timed to the lines (a leading number is the start in seconds; otherwise space them evenly over the cut, about 2–4 s each, never overlapping):\n{c}"),
            };
            format!(
                "Read project_overview for the canvas size and the length of the cut. Add the title \"{title}\" with clip_addText at 0 s for about 3 s on a video track above the pictures \
                 (track_add kind=video puts a new one on top). Style: {style}. Use the style object (fontSize in project pixels, fontWeight, color, background, shadow) and y to place it; \
                 keep text inside the frame with a margin of about 5 % of the canvas. Fade it in and out with clip_update fadeIn/fadeOut. {captions}\n\
                 Do all the captions in one project_batch so they are one undo step. Finish by listing each text clip with its start and end.",
                title = arg(a, "title", "Untitled"),
                style = arg(a, "style", "clean white sans serif, centred in the lower third, with a soft shadow"),
            )
        },
    },
    Prompt {
        name: "generate-b-roll",
        description: "Generate b-roll shots for a subject and lay them into the cut where they help.",
        arguments: &[
            ("subject", "What the b-roll should show, e.g. \"a rainy Tokyo street at night\"", true),
            ("count", "How many shots (default 3)", false),
            ("seconds", "Length of each shot in seconds (default 5)", false),
        ],
        render: |a| {
            format!(
                "Generate {count} b-roll shots of {subject}, about {seconds} s each. First read project_overview and generate_providers; pick a ready video model with \
                 generate_models task=text_to_video (or follow settings.generate.videoModel). If no provider is ready, stop and tell the person which keys to add in Settings — never ask for a key. \
                 Write each prompt as a shot description (subject, camera move, light, lens), varied across the shots, matching the project's aspect ratio. \
                 Submit each with generate_submit video=true duration={seconds}, placed on a free video track where the cut needs cover (gaps, long talking shots), or place=library \
                 if unsure. Use wait=true one at a time, or submit all and follow them with generate_jobs / generate_wait. When a shot is close but not right, use generate_regenerate \
                 variation=true rather than starting over. Report each shot's prompt, model and where it landed. Generation spends the person's credits: don't make more than asked.",
                count = arg(a, "count", "3"),
                subject = arg(a, "subject", "the project's subject"),
                seconds = arg(a, "seconds", "5"),
            )
        },
    },
    Prompt {
        name: "review-the-cut",
        description: "Review the open cut for pacing, gaps, missing media, levels and text, and propose or apply fixes.",
        arguments: &[("apply", "true to apply the fixes, otherwise only propose them", false)],
        render: |a| {
            let apply = if arg(a, "apply", "false") == "true" {
                "Apply the fixes, grouping related edits in project_batch so each is one undo step, and say what you changed."
            } else {
                "Don't change anything yet: list each issue with its time and the exact commands you would run."
            };
            format!(
                "Review this cut. Call project_overview and read problems first (missing files, clips still generating, hidden or muted tracks). Then check: gaps on the main \
                 video track (timeline_closeGap), shots that run long or cut too fast for the material, jarring jumps between generated and filmed shots (generate_bridge can fill \
                 a transition), audio clips that overlap or are too loud (clip_update volume, fadeIn, fadeOut), text that runs off the canvas or overlaps other text, and the \
                 ending. Use clip_get or media_get where you need details, and in live mode ui_screenshot to see the window. {apply}"
            )
        },
    },
    Prompt {
        name: "motion-design",
        description: "Design motion graphics for the cut: animated titles, lower thirds, callouts, data and transitions, drawn by kimchi and fully editable.",
        arguments: &[
            ("brief", "What to make, e.g. \"an intro title, a lower third for each speaker, an animated chart of the numbers\"", true),
            ("style", "Look and feel, e.g. \"bold, minimal, coral and near-black\" (default: the project's style or clean and modern)", false),
        ],
        render: |a| {
            format!(
                "Motion design brief: {brief}. Style: {style}.\n\
                 1. Read project_overview (canvas size, length, what is on which track) and motion_guide (formats, properties, easings).\n\
                 2. Start from motion_templates where one fits (lowerThird, titleCard, kineticType, counter, barChart, logoReveal, callout, quote, subscribe, aurora, wipe, \
                 title3d, logoSpin3d, turntable, shapes3d) with motion_addTemplate and your values; write your own scene with motion_add when none does. Put motion \
                 clips on a video track above the pictures they go over (track_add kind=video adds one on top).\n\
                 3. Animate with intent: entrances easeOut 0.3–0.8 s, exits easeIn, related elements staggered 0.05–0.15 s, nothing moving without a reason. Keep text \
                 inside the safe area (5 % margins), large enough, and readable over the picture (a plate, shadow or glow).\n\
                 4. Animate existing clips too where it helps (clip_animate kenBurns on stills, popIn on logos, fades between sections).\n\
                 5. Check every piece with project_renderFrame times=[…] at its entrance, middle and exit; fix what you see with motion_setLayer / motion_setKeyframes / \
                 motion_setTemplate. Group related edits in project_batch.\n\
                 Finish by listing each motion clip with its time, what it shows and how it moves.",
                brief = arg(a, "brief", "an animated title for the cut"),
                style = arg(a, "style", "clean and modern, matching the footage"),
            )
        },
    },
    Prompt {
        name: "3d-scene",
        description: "Build a 3D shot (a product turntable, a 3D title, an abstract backdrop) as a motion clip, with camera moves and lighting.",
        arguments: &[
            ("idea", "The shot, e.g. \"our logo in brushed metal spinning above a dark floor\"", true),
            ("seconds", "Length in seconds (default 5)", false),
        ],
        render: |a| {
            format!(
                "Make this 3D shot, about {seconds} s long: {idea}.\n\
                 Read motion_guide topic=3d. Start from a 3D template if one is close (title3d, logoSpin3d, turntable with a .glb model, shapes3d) or write the scene \
                 with motion_add: a camera with a purposeful move (a slow push in, an orbit, a reveal), a key light plus a fill or rim (or the defaults), a floor plane \
                 to catch the shadow when things sit on something, materials with intent (metallic/roughness, emissive accents), and animation on objects and camera with \
                 easeInOut curves. Models can be media items or file paths (.glb/.gltf); pictures can texture objects or stand as image cards.\n\
                 Check it with project_renderFrame times=[0.5, {mid}, {late}]: framing, lighting, nothing clipping the camera or leaving the frame unintentionally. Adjust \
                 with motion_setLayer (an object or light), motion_setKeyframes (id \"camera\" for the camera) or motion_update. Report what the shot shows and how it moves.",
                idea = arg(a, "idea", "a 3D title"),
                seconds = arg(a, "seconds", "5"),
                mid = arg(a, "seconds", "5").parse::<f64>().map(|s| format!("{}", (s / 2.0 * 10.0).round() / 10.0)).unwrap_or_else(|_| "2.5".into()),
                late = arg(a, "seconds", "5").parse::<f64>().map(|s| format!("{}", ((s - 0.5) * 10.0).round() / 10.0)).unwrap_or_else(|_| "4.5".into()),
            )
        },
    },
];
