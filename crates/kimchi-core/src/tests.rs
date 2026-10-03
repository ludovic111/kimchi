use crate::*;

fn video_asset(duration: f64) -> Asset {
    Asset {
        id: new_id(),
        name: "clip.mp4".into(),
        kind: MediaKind::Video,
        path: "/tmp/clip.mp4".into(),
        meta: MediaMeta { duration: Some(duration), has_video: true, has_audio: true, ..Default::default() },
        origin: AssetOrigin::Imported,
        created_at: chrono::Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
    }
}

fn setup() -> (Project, Asset) {
    let mut p = Project::new("Test", ProjectSettings::default());
    let a = video_asset(10.0);
    p.apply(&Edit::AddAsset { asset: a.clone() }).unwrap();
    (p, a)
}

fn insert(p: &mut Project, a: &Asset, start: f64) -> Id {
    p.apply(&Edit::InsertAsset { asset_id: a.id, track_id: None, start }).unwrap().created_clips[0]
}

#[test]
fn insert_uses_asset_duration_and_free_track() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 2.0);
    let c = p.clip(id).unwrap();
    assert_eq!((c.start, c.duration), (2.0, 10.0));
    // Overlapping insert goes to a fresh track rather than overwriting.
    let id2 = insert(&mut p, &a, 5.0);
    let (t1, _) = p.locate_clip(id).unwrap();
    let (t2, _) = p.locate_clip(id2).unwrap();
    assert_ne!(t1, t2);
}

#[test]
fn split_keeps_source_continuity() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    let out = p.apply(&Edit::Split { time: 4.0, clip_ids: None }).unwrap();
    let right = p.clip(out.created_clips[0]).unwrap();
    assert_eq!((right.start, right.duration, right.in_point), (4.0, 6.0, 4.0));
    assert_eq!(p.clip(id).unwrap().duration, 4.0);
}

#[test]
fn trim_is_bounded_by_source_and_neighbours() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    p.apply(&Edit::TrimClip { clip_id: id, edge: Edge::End, time: 50.0 }).unwrap();
    assert_eq!(p.clip(id).unwrap().duration, 10.0);
    p.apply(&Edit::TrimClip { clip_id: id, edge: Edge::Start, time: 3.0 }).unwrap();
    let c = p.clip(id).unwrap();
    assert_eq!((c.start, c.duration, c.in_point), (3.0, 7.0, 3.0));
    // Can extend back to the source's first frame, not further.
    p.apply(&Edit::TrimClip { clip_id: id, edge: Edge::Start, time: -5.0 }).unwrap();
    let c = p.clip(id).unwrap();
    assert_eq!((c.start, c.in_point), (0.0, 0.0));
}

#[test]
fn moving_onto_a_clip_overwrites_it() {
    let (mut p, a) = setup();
    let track = p.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap().id;
    let mut text = Clip::new("Text", 0.0, 10.0, ClipContent::Text { style: TextStyle::default() });
    text.id = new_id();
    let base = p.apply(&Edit::AddClip { track_id: Some(track), clip: text }).unwrap().created_clips[0];
    let mover = p.apply(&Edit::AddClip { track_id: None, clip: Clip::new("Solid", 20.0, 2.0, ClipContent::Solid { color: "#fff".into() }) }).unwrap().created_clips[0];
    p.apply(&Edit::MoveClips { moves: vec![ClipMove { clip_id: mover, track_id: track, start: 4.0 }] }).unwrap();
    let t = p.track(track).unwrap();
    assert_eq!(t.clips.len(), 3, "base is split around the moved clip");
    assert_eq!(p.clip(base).unwrap().duration, 4.0);
    let _ = a;
}

#[test]
fn paste_places_new_copies_whole_or_not_at_all() {
    let (mut p, a) = setup();
    let track = p.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap().id;
    let first = p.apply(&Edit::InsertAsset { asset_id: a.id, track_id: Some(track), start: 0.0 }).unwrap().created_clips[0];
    let mut copy = p.clip(first).unwrap().clone();
    copy.start = 30.0;
    let made = p.apply(&Edit::PasteClips { clips: vec![TrackClip { track_id: track, clip: copy.clone() }] }).unwrap().created_clips;
    assert_eq!(made.len(), 1);
    assert_ne!(made[0], first, "a paste gets a new id");
    assert_eq!(p.clip(made[0]).unwrap().start, 30.0);
    assert!(p.clip(first).is_some(), "the original stays");
    // A clip whose media isn't in the project lands nowhere.
    copy.content = ClipContent::Media { asset_id: new_id() };
    let before = p.clips().count();
    assert!(p.apply(&Edit::PasteClips { clips: vec![TrackClip { track_id: track, clip: copy }] }).is_err());
    assert_eq!(p.clips().count(), before);
}

#[test]
fn ripple_delete_closes_the_hole() {
    let (mut p, a) = setup();
    let track = p.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap().id;
    let first = p.apply(&Edit::InsertAsset { asset_id: a.id, track_id: Some(track), start: 0.0 }).unwrap().created_clips[0];
    let second = p.apply(&Edit::InsertAsset { asset_id: a.id, track_id: Some(track), start: 10.0 }).unwrap().created_clips[0];
    p.apply(&Edit::DeleteClips { clip_ids: vec![first], ripple: true }).unwrap();
    assert_eq!(p.clip(second).unwrap().start, 0.0);
}

#[test]
fn undo_redo_and_background_edits() {
    let (p, a) = setup();
    let mut ed = Editor::new(p);
    let pending = Clip::new("gen", 1.0, 5.0, ClipContent::Pending { job_id: "job1".into(), kind: MediaKind::Video, prompt: "a cat".into(), model_name: "m".into() });
    ed.apply(&Edit::AddClip { track_id: None, clip: pending }, None).unwrap();
    ed.apply(&Edit::Split { time: 3.0, clip_ids: None }, None).unwrap();
    // The job finishes: both halves become real media, in every history state.
    ed.apply(&Edit::ResolvePending { job_id: "job1".into(), asset_id: a.id }, None).unwrap();
    assert!(ed.project().clips().all(|(_, c)| matches!(c.content, ClipContent::Media { .. })));
    assert!(ed.undo());
    assert!(ed.project().clips().all(|(_, c)| matches!(c.content, ClipContent::Media { .. })));
    assert!(ed.redo());
}

#[test]
fn slider_drags_coalesce() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    let mut ed = Editor::new(p);
    for v in [0.5, 0.6, 0.7] {
        ed.apply(&Edit::UpdateClip { clip_id: id, patch: ClipPatch { volume: Some(v), ..Default::default() } }, Some("vol")).unwrap();
    }
    // The slider's final value, the same as the last one, adds no step.
    ed.apply(&Edit::UpdateClip { clip_id: id, patch: ClipPatch { volume: Some(0.7), ..Default::default() } }, None).unwrap();
    assert!(ed.undo());
    assert_eq!(ed.project().clip(id).unwrap().volume, 1.0);
    assert!(!ed.can_undo());
}

#[test]
fn speed_changes_timeline_length() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    p.apply(&Edit::UpdateClip { clip_id: id, patch: ClipPatch { speed: Some(2.0), ..Default::default() } }).unwrap();
    assert_eq!(p.clip(id).unwrap().duration, 5.0);
}

#[test]
fn library_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path());
    let (p, _) = setup();
    lib.save(&p).unwrap();
    assert_eq!(lib.load(p.id).unwrap(), p);
    assert_eq!(lib.list().len(), 1);
}

#[test]
fn titles_land_on_top_footage_at_the_bottom() {
    let (mut p, a) = setup();
    p.apply(&Edit::AddTrack { kind: TrackKind::Video, index: Some(0) }).unwrap();
    let footage = insert(&mut p, &a, 0.0);
    let title = p.apply(&Edit::AddClip { track_id: None, clip: Clip::new("T", 0.0, 2.0, ClipContent::Text { style: TextStyle::default() }) }).unwrap().created_clips[0];
    let (ft, _) = p.locate_clip(footage).unwrap();
    let (tt, _) = p.locate_clip(title).unwrap();
    assert!(tt < ft, "title track {tt} should be above footage track {ft}");
}

#[test]
fn batches_are_one_step_and_roll_back() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    let mut ed = Editor::new(p);
    ed.begin_batch("project.batch", "agent");
    ed.apply(&Edit::UpdateClip { clip_id: id, patch: ClipPatch { volume: Some(0.5), ..Default::default() } }, None).unwrap();
    ed.apply(&Edit::AddMarker { time: 1.0, label: "a".into() }, None).unwrap();
    ed.end_batch();
    assert_eq!(ed.undo_steps().len(), 1);
    assert_eq!(ed.undo_steps()[0].source, "agent");
    assert!(ed.undo());
    assert_eq!(ed.project().clip(id).unwrap().volume, 1.0);
    assert!(ed.project().markers.is_empty());

    ed.begin_batch("project.batch", "agent");
    ed.apply(&Edit::AddMarker { time: 1.0, label: "b".into() }, None).unwrap();
    ed.rollback_batch();
    assert!(ed.project().markers.is_empty());
    assert!(!ed.can_undo());
    assert!(ed.can_redo());
}

#[test]
fn checkpoints_revert_as_one_undoable_step() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    let mut ed = Editor::new(p);
    let cp = ed.checkpoint();
    ed.apply(&Edit::DeleteClips { clip_ids: vec![id], ripple: false }, None).unwrap();
    assert!(ed.project().clip(id).is_none());
    assert!(ed.revert_to(cp).unwrap());
    assert!(ed.project().clip(id).is_some());
    assert!(ed.undo());
    assert!(ed.project().clip(id).is_none());
    // Ids are unique in the process: another editor's checkpoint isn't this one's.
    let mut other = Editor::new(Project::new("Other", ProjectSettings::default()));
    let theirs = other.checkpoint();
    assert_ne!(theirs, cp);
    assert!(!ed.revert_to(theirs).unwrap());
}

#[test]
fn a_failed_edit_changes_nothing() {
    let (mut p, a) = setup();
    let first = insert(&mut p, &a, 0.0);
    let second = insert(&mut p, &a, 20.0);
    let video = p.locate_clip(first).map(|(ti, _)| p.tracks[ti].id).unwrap();
    let audio = p.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().id;
    let mut ed = Editor::new(p);
    let before = ed.project().clone();
    // The first move is fine, the second can't go on an audio track.
    let moves = vec![ClipMove { clip_id: first, track_id: video, start: 40.0 }, ClipMove { clip_id: second, track_id: audio, start: 0.0 }];
    assert!(ed.apply(&Edit::MoveClips { moves }, None).is_err());
    assert_eq!(ed.project(), &before);
    assert_eq!(ed.project().clip(first).unwrap().start, 0.0);
    assert!(!ed.can_undo());
}

#[test]
fn a_nested_rollback_keeps_the_outer_batch() {
    let (p, _) = setup();
    let mut ed = Editor::new(p);
    ed.begin_batch("project.batch", "agent");
    ed.apply(&Edit::AddMarker { time: 1.0, label: "outer".into() }, None).unwrap();
    ed.begin_batch("clip.setEffects", "agent");
    ed.apply(&Edit::AddMarker { time: 2.0, label: "inner".into() }, None).unwrap();
    ed.rollback_batch();
    assert!(ed.in_batch());
    assert_eq!(ed.project().markers.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(), ["outer"]);
    ed.end_batch();
    assert!(!ed.in_batch());
    assert_eq!(ed.undo_steps().len(), 1);
    // A rolled-back inner batch that made the outer's first change takes the step with it.
    ed.begin_batch("project.batch", "agent");
    ed.begin_batch("inner", "agent");
    ed.apply(&Edit::AddMarker { time: 3.0, label: "x".into() }, None).unwrap();
    ed.rollback_batch();
    ed.apply(&Edit::AddMarker { time: 4.0, label: "y".into() }, None).unwrap();
    ed.end_batch();
    assert_eq!(ed.undo_steps().len(), 2);
    assert!(ed.undo());
    assert_eq!(ed.project().markers.len(), 1);
}

#[test]
fn outsiders_cant_edit_into_an_open_batch() {
    let (p, _) = setup();
    let mut ed = Editor::new(p);
    ed.begin_batch("project.batch", "agent");
    ed.set_outsider(true);
    assert_eq!(ed.apply(&Edit::AddMarker { time: 1.0, label: "w".into() }, None), Err(EditError::Busy("agent".into())));
    ed.set_outsider(false);
    ed.apply(&Edit::AddMarker { time: 1.0, label: "a".into() }, None).unwrap();
    ed.end_batch();
    ed.set_outsider(true);
    ed.apply(&Edit::AddMarker { time: 2.0, label: "w".into() }, None).unwrap();
}

#[test]
fn slivers_times_and_gaps_are_handled() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    let track = p.locate_clip(id).map(|(ti, _)| p.tracks[ti].id).unwrap();
    // Overwriting all but a hundredth of a second drops the sliver.
    let over = Clip::new("T", 0.01, 20.0, ClipContent::Solid { color: "#000000".into() });
    p.apply(&Edit::AddClip { track_id: Some(track), clip: over }).unwrap();
    assert!(p.clip(id).is_none());
    // A clip already a sliver against its neighbour can't be trimmed, and doesn't panic.
    let t = p.track_mut(track).unwrap();
    let mut sliver = Clip::new("s", 20.01, 0.005, ClipContent::Solid { color: "#000000".into() });
    sliver.id = new_id();
    let sid = sliver.id;
    t.clips.push(sliver);
    p.apply(&Edit::TrimClip { clip_id: sid, edge: Edge::Start, time: 25.0 }).unwrap();
    // Close gap inside a clip.
    let e = p.apply(&Edit::CloseGap { track_id: track, time: 5.0 }).unwrap_err();
    assert!(e.to_string().contains("no gap"), "{e}");
    for edit in [
        Edit::AddMarker { time: f64::INFINITY, label: "x".into() },
        Edit::AddMarker { time: 1e300, label: "x".into() },
        Edit::InsertAsset { asset_id: a.id, track_id: None, start: f64::NAN },
        Edit::MoveClips { moves: vec![ClipMove { clip_id: sid, track_id: track, start: 1e12 }] },
    ] {
        assert!(p.apply(&edit).is_err(), "{edit:?}");
    }
    // Faster than the media left after the in point allows.
    let id = insert(&mut p, &a, 100.0);
    p.clip_mut(id).unwrap().in_point = 9.999;
    p.clip_mut(id).unwrap().duration = 0.001;
    assert!(p.apply(&Edit::UpdateClip { clip_id: id, patch: ClipPatch { speed: Some(16.0), ..Default::default() } }).is_err());
}

#[test]
fn paste_and_undo_preserve_motion_scenes_and_keyframes() {
    let mut p = Project::new("animated clipboard", ProjectSettings::default());
    let scene = templates::TEMPLATES[0].build(&Default::default(), templates::Ctx { width: 1920., height: 1080., duration: 3. }).unwrap();
    let mut original = Clip::new("motion", 0.0, 3.0, ClipContent::Motion { scene, template: None });
    original.keyframes.insert("x".into(), vec![Keyframe::new(0.0, 0.0, Default::default()), Keyframe::new(1.0, 100.0, Default::default())]);
    let track = p.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap().id;
    p.track_mut(track).unwrap().clips.push(original.clone());
    let mut editor = Editor::new(p);
    let mut copy = original.clone();
    copy.start = 5.0;
    editor.apply(&Edit::PasteClips { clips: vec![TrackClip { track_id: track, clip: copy }] }, None).unwrap();
    let pasted = editor.project().clips().find(|(_, c)| c.start == 5.0).unwrap().1;
    assert_ne!(pasted.id, original.id);
    assert_eq!(pasted.content, original.content);
    assert_eq!(pasted.keyframes, original.keyframes);
    assert!(editor.undo());
    assert_eq!(editor.project().clips().count(), 1);
    assert!(editor.redo());
    assert_eq!(editor.project().clips().find(|(_, c)| c.start == 5.0).unwrap().1.content, original.content);
}

fn update(p: &mut Project, id: Id, patch: ClipPatch) {
    p.apply(&Edit::UpdateClip { clip_id: id, patch }).unwrap();
}

#[test]
fn reversed_clips_split_and_trim_from_the_other_end() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    p.apply(&Edit::TrimClip { clip_id: id, edge: Edge::Start, time: 2.0 }).unwrap();
    // Source 2–10 s on the timeline at 2–10 s; reversed, the first frame shows source 10 s.
    update(&mut p, id, ClipPatch { reverse: Some(true), ..Default::default() });
    let c = p.clip(id).unwrap().clone();
    assert_eq!((c.source_time(2.0), c.source_time(10.0)), (10.0, 2.0));
    // The left half keeps the end of the source, the right half its start.
    let right = p.apply(&Edit::Split { time: 4.0, clip_ids: Some(vec![id]) }).unwrap().created_clips[0];
    let (l, r) = (p.clip(id).unwrap().clone(), p.clip(right).unwrap().clone());
    assert_eq!((l.in_point, l.duration, l.source_time(2.0)), (8.0, 2.0, 10.0));
    assert_eq!((r.in_point, r.duration, r.source_time(4.0)), (2.0, 6.0, 8.0));
    // Dragging the right half's end out reaches back towards source 0.
    p.apply(&Edit::TrimClip { clip_id: right, edge: Edge::End, time: 30.0 }).unwrap();
    let r = p.clip(right).unwrap();
    assert_eq!((r.in_point, r.end()), (0.0, 12.0));
    // The left half's start can't go earlier: its first frame is the source's last.
    p.apply(&Edit::TrimClip { clip_id: id, edge: Edge::Start, time: 0.0 }).unwrap();
    assert_eq!(p.clip(id).unwrap().start, 2.0);
}

#[test]
fn stills_cant_play_backwards() {
    let mut p = Project::new("T", ProjectSettings::default());
    let clip = Clip::new("s", 0.0, 2.0, ClipContent::Solid { color: "#000000".into() });
    let id = p.apply(&Edit::AddClip { track_id: None, clip }).unwrap().created_clips[0];
    let err = p.apply(&Edit::UpdateClip { clip_id: id, patch: ClipPatch { reverse: Some(true), ..Default::default() } }).unwrap_err();
    assert!(matches!(err, EditError::Invalid(_)));
}

#[test]
fn splitting_drops_the_transition_from_the_right_half_and_effects_follow() {
    let (mut p, a) = setup();
    let id = insert(&mut p, &a, 0.0);
    let effects = Effects { saturation: -1.0, ..Default::default() };
    let transition = Some(Transition::new(TransitionKind::Dissolve, 1.0));
    update(&mut p, id, ClipPatch { effects: Some(effects.clone()), transition: Some(transition.clone()), ..Default::default() });
    let right = p.apply(&Edit::Split { time: 5.0, clip_ids: None }).unwrap().created_clips[0];
    let r = p.clip(right).unwrap();
    assert_eq!((r.transition.clone(), r.effects.clone()), (None, effects));
    assert_eq!(p.clip(id).unwrap().transition, transition);
    // Effect keyframes animate the value.
    let mut keys = Keyframes::new();
    keys.insert("saturation".into(), vec![Keyframe::new(0.0, 0.0, Easing::Linear), Keyframe::new(2.0, 1.0, Easing::Linear)]);
    update(&mut p, id, ClipPatch { keyframes: Some(keys), ..Default::default() });
    assert!((p.clip(id).unwrap().effects_at(1.0).saturation - 0.5).abs() < 1e-9);
}
