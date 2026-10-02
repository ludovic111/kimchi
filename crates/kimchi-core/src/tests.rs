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
    assert!(ed.revert_to(cp));
    assert!(ed.project().clip(id).is_some());
    assert!(ed.undo());
    assert!(ed.project().clip(id).is_none());
}
