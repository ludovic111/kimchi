//! Round trips kimchi → format → kimchi for every writable format, real-looking files from the
//! other apps, and bad input.

use std::path::Path;

use kimchi_core::{Asset, AssetOrigin, Clip, ClipContent, MediaKind, MediaMeta, Project, ProjectSettings, TextStyle, Track, TrackKind, Transition, TransitionKind, new_id};

use super::*;

fn asset(dir: &Path, name: &str, kind: MediaKind) -> Asset {
    let path = dir.join(name);
    std::fs::write(&path, b"media").unwrap();
    Asset {
        id: new_id(),
        name: name.into(),
        kind,
        path: path.to_string_lossy().into_owned(),
        meta: MediaMeta { duration: Some(60.0), width: Some(1920), height: Some(1080), fps: Some(30.0), has_video: kind != MediaKind::Audio, has_audio: kind != MediaKind::Image, ..Default::default() },
        origin: AssetOrigin::Imported,
        created_at: chrono::Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
        beats: None,
    }
}

/// A cut at 29.97: two clips with a dissolve on V1, a title on V2, music on A1, a marker.
fn sample(dir: &Path) -> Project {
    let mut p = Project::new("Holiday cut", ProjectSettings { fps: 30000.0 / 1001.0, ..Default::default() });
    let rate = time::Rate::from_fps(p.settings.fps);
    let f = |n: i64| rate.seconds(n as f64);
    let a = asset(dir, "beach shot.mov", MediaKind::Video);
    let b = asset(dir, "city.mov", MediaKind::Video);
    let m = asset(dir, "song.wav", MediaKind::Audio);
    let mut c1 = Clip::new("beach", f(0), f(150), ClipContent::Media { asset_id: a.id });
    c1.in_point = f(300);
    c1.transform.opacity = 0.8;
    let mut c2 = Clip::new("city", f(150), f(90), ClipContent::Media { asset_id: b.id });
    c2.in_point = f(30);
    c2.speed = 2.0;
    c2.transition = Some(Transition::new(TransitionKind::Dissolve, f(30)));
    let title = Clip::new("Hello", f(30), f(60), ClipContent::Text { style: TextStyle { content: "Hello".into(), ..Default::default() } });
    let mut music = Clip::new("song", f(0), f(240), ClipContent::Media { asset_id: m.id });
    music.volume = 0.5;
    p.assets = vec![a, b, m];
    let mut v2 = Track::new(TrackKind::Video, "Titles");
    v2.clips = vec![title];
    let mut v1 = Track::new(TrackKind::Video, "Video 1");
    v1.clips = vec![c1, c2];
    let mut a1 = Track::new(TrackKind::Audio, "Music");
    a1.clips = vec![music];
    p.tracks = vec![v2, v1, a1];
    p.markers.push(kimchi_core::Marker { id: new_id(), time: f(60), label: "Look here".into(), color: "#ff3b30".into() });
    p
}

fn media_clips(p: &Project, kind: TrackKind) -> Vec<&Clip> {
    let mut v: Vec<&Clip> = p.tracks.iter().filter(|t| t.kind == kind).flat_map(|t| t.clips.iter()).filter(|c| matches!(c.content, ClipContent::Media { .. })).collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    v
}

fn same_frame(a: f64, b: f64, fps: f64) -> bool {
    ((a - b) * fps).abs() < 0.5
}

fn round_trip(format: &str, ext: &str) -> (Project, Project, Report, Report) {
    let dir = tempfile::tempdir().unwrap();
    let p = sample(dir.path());
    let file = dir.path().join(format!("cut.{ext}"));
    let out = export(&p, format, &file).unwrap_or_else(|e| panic!("{format}: {e}"));
    let back = import(&file, None).unwrap_or_else(|e| panic!("{format}: {e}"));
    let _keep = dir.keep();
    (p, back.project, out, back.report)
}

fn check_cut(format: &str, p: &Project, back: &Project) {
    let fps = p.settings.fps;
    assert!((back.settings.fps - fps).abs() < 1e-9, "{format}: fps {}", back.settings.fps);
    let (want, got) = (media_clips(p, TrackKind::Video), media_clips(back, TrackKind::Video));
    assert_eq!(got.len(), want.len(), "{format}: video clips {got:#?}");
    for (w, g) in want.iter().zip(&got) {
        assert!(same_frame(w.start, g.start, fps), "{format}: start {} vs {}", w.start, g.start);
        assert!(same_frame(w.duration, g.duration, fps), "{format}: duration {} vs {}", w.duration, g.duration);
        assert!(same_frame(w.in_point, g.in_point, fps), "{format}: in point {} vs {}", w.in_point, g.in_point);
        assert!((w.speed - g.speed).abs() < 1e-6, "{format}: speed");
    }
    assert!(got[1].transition.as_ref().is_some_and(|t| t.kind == TransitionKind::Dissolve && same_frame(t.duration, p.tracks[1].clips[1].transition.as_ref().unwrap().duration, fps)), "{format}: transition {:?}", got[1].transition);
    let music = media_clips(back, TrackKind::Audio);
    assert_eq!(music.len(), 1, "{format}: audio {music:#?}");
    assert!(music[0].path_ok(back), "{format}: music asset");
}

trait PathOk {
    fn path_ok(&self, p: &Project) -> bool;
}

impl PathOk for Clip {
    fn path_ok(&self, p: &Project) -> bool {
        self.asset_id().and_then(|id| p.asset(id)).is_some_and(|a| Path::new(&a.path).is_file())
    }
}

#[test]
fn otio_round_trip_is_lossless() {
    let (p, back, _, report) = round_trip("otio", "otio");
    check_cut("otio", &p, &back);
    assert!(report.missing_media.is_empty(), "{report:?}");
    // kimchi's own data comes back: the title, opacity, volume, track names, the marker.
    assert!(back.clips().any(|(_, c)| matches!(&c.content, ClipContent::Text { style } if style.content == "Hello")));
    let beach = back.clips().find(|(_, c)| c.name == "beach").unwrap().1;
    assert_eq!(beach.transform.opacity, 0.8);
    assert!(back.tracks.iter().any(|t| t.name == "Music"));
    assert_eq!(back.markers.len(), 1);
    assert_eq!(back.markers[0].color, "#ff3b30");
    assert!(back.clips().any(|(_, c)| c.volume == 0.5));
    // The video clips' sound came back as their own, not as extra audio clips.
    assert!(!beach.audio.muted);
}

#[test]
fn otio_bundles() {
    for ext in ["otioz", "otiod"] {
        let (p, back, out, _) = round_trip("otio", ext);
        check_cut(ext, &p, &back);
        assert!(out.kept.iter().any(|k| k.contains("inside the bundle")), "{out:?}");
    }
}

#[test]
fn xmeml_round_trip() {
    let (p, back, _, report) = round_trip("xmeml", "xml");
    check_cut("xmeml", &p, &back);
    let beach = back.clips().find(|(_, c)| c.name == "beach").unwrap().1;
    assert!((beach.transform.opacity - 0.8).abs() < 1e-6);
    assert!(!beach.audio.muted, "sound linked back to its picture");
    assert!(back.clips().any(|(_, c)| matches!(&c.content, ClipContent::Text { style } if style.content == "Hello")));
    assert!(back.clips().any(|(_, c)| (c.volume - 0.5).abs() < 1e-6));
    assert_eq!(back.markers.len(), 1);
    assert!(report.missing_media.is_empty());
}

#[test]
fn fcpxml_round_trip() {
    for ext in ["fcpxml", "fcpxmld"] {
        let (p, back, _, _) = round_trip("fcpxml", ext);
        check_cut(ext, &p, &back);
        let beach = back.clips().find(|(_, c)| c.name == "beach").unwrap().1;
        assert!((beach.transform.opacity - 0.8).abs() < 1e-4);
        assert!(back.clips().any(|(_, c)| matches!(&c.content, ClipContent::Text { style } if style.content == "Hello")));
        assert!(back.clips().any(|(_, c)| (c.volume - 0.5).abs() < 1e-3));
        assert_eq!(back.markers.len(), 1);
    }
}

#[test]
fn edl_round_trip_keeps_the_main_track() {
    let (p, back, out, _) = round_trip("edl", "edl");
    let fps = p.settings.fps;
    let got = media_clips(&back, TrackKind::Video);
    assert_eq!(got.len(), 2, "{got:#?}");
    for (w, g) in media_clips(&p, TrackKind::Video).iter().zip(&got) {
        assert!(same_frame(w.start, g.start, fps) && same_frame(w.duration, g.duration, fps) && same_frame(w.in_point, g.in_point, fps), "{w:?} vs {g:?}");
    }
    assert!(got[1].transition.is_some());
    assert!(out.dropped.iter().any(|d| d.contains("titles")), "{out:?}");
}

#[test]
fn reads_premiere_xml() {
    let dir = tempfile::tempdir().unwrap();
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="4">
  <sequence id="sequence-2">
    <name>Sequence 01</name>
    <duration>250</duration>
    <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
    <media>
      <video>
        <format><samplecharacteristics><width>1280</width><height>720</height></samplecharacteristics></format>
        <track>
          <clipitem id="clipitem-1">
            <name>A001.mov</name><enabled>TRUE</enabled><duration>500</duration>
            <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
            <start>0</start><end>125</end><in>50</in><out>175</out>
            <file id="file-1">
              <name>A001.mov</name>
              <pathurl>file://localhost/Volumes/Media/A001%20take.mov</pathurl>
              <rate><timebase>25</timebase><ntsc>FALSE</ntsc></rate>
              <duration>500</duration>
              <media><video><samplecharacteristics><width>1280</width><height>720</height></samplecharacteristics></video><audio/></media>
            </file>
            <filter><effect><name>Basic Motion</name><effectid>basic</effectid>
              <parameter><parameterid>scale</parameterid><value>150</value></parameter>
              <parameter><parameterid>center</parameterid><value><horiz>0.25</horiz><vert>0</vert></value></parameter>
            </effect></filter>
            <filter><effect><name>Gaussian Blur</name><effectid>gaussianblur</effectid></effect></filter>
          </clipitem>
          <generatoritem id="gen-1"><name>Color</name><start>125</start><end>250</end><in>0</in><out>125</out>
            <effect><name>Color</name><effectid>Color</effectid><effecttype>generator</effecttype>
              <parameter><parameterid>fillcolor</parameterid><value><alpha>255</alpha><red>255</red><green>0</green><blue>0</blue></value></parameter>
            </effect>
          </generatoritem>
          <enabled>TRUE</enabled><locked>FALSE</locked>
        </track>
      </video>
      <audio>
        <track>
          <clipitem id="clipitem-2"><name>A001.mov</name><start>0</start><end>125</end><in>50</in><out>175</out><file id="file-1"/>
            <filter><effect><name>Audio Levels</name><effectid>audiolevels</effectid><parameter><parameterid>level</parameterid><value>0.5</value></parameter></effect></filter>
          </clipitem>
        </track>
      </audio>
    </media>
    <marker><name>Chapter</name><in>100</in><out>-1</out></marker>
  </sequence>
</xmeml>"#;
    let file = dir.path().join("seq.xml");
    std::fs::write(&file, xml).unwrap();
    assert_eq!(detect(&file).map(|f| f.id), Some("xmeml"));
    let imp = import(&file, None).unwrap();
    let p = &imp.project;
    assert_eq!((p.settings.width, p.settings.height, p.settings.fps), (1280, 720, 25.0));
    let v = media_clips(p, TrackKind::Video);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].in_point, 2.0);
    assert!((v[0].transform.scale - 1.5).abs() < 1e-9 && (v[0].transform.x - 320.0).abs() < 1e-9);
    assert!((v[0].volume - 0.5).abs() < 1e-9 && !v[0].audio.muted);
    assert_eq!(p.asset(v[0].asset_id().unwrap()).unwrap().path, "/Volumes/Media/A001 take.mov");
    assert!(p.clips().any(|(_, c)| matches!(&c.content, ClipContent::Solid { color } if color == "#ff0000")));
    assert_eq!(imp.report.missing_media, vec!["/Volumes/Media/A001 take.mov".to_string()]);
    assert!(imp.report.dropped.iter().any(|d| d.contains("Gaussian Blur")), "{:?}", imp.report);
    assert_eq!(p.markers[0].time, 4.0);
}

#[test]
fn reads_final_cut_fcpxml() {
    let dir = tempfile::tempdir().unwrap();
    let doc = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE fcpxml>
<fcpxml version="1.11">
  <resources>
    <format id="r1" name="FFVideoFormat1080p2398" frameDuration="1001/24000s" width="1920" height="1080" colorSpace="1-1-1 (Rec. 709)"/>
    <asset id="r2" name="Interview" uid="ABC" start="3600s" duration="120120/24000s" hasVideo="1" format="r1" hasAudio="1" audioSources="1" audioChannels="2" audioRate="48000">
      <media-rep kind="original-media" src="file:///Users/me/Movies/Interview.mov"/>
    </asset>
    <asset id="r3" name="Music" start="0s" duration="60s" hasAudio="1" audioSources="1" audioChannels="2">
      <media-rep kind="original-media" src="file:///Users/me/Music/track.m4a"/>
    </asset>
    <effect id="r4" name="Basic Title" uid=".../Titles.localized/Bumper:Opener.localized/Basic Title.localized/Basic Title.moti"/>
    <effect id="r5" name="Cross Dissolve" uid="FxPlug:4731E73A-8DAC-4113-9A30-AE85B1761265"/>
  </resources>
  <library>
    <event name="Day 1">
      <project name="Interview cut">
        <sequence format="r1" duration="240240/24000s" tcStart="0s" tcFormat="NDF" audioLayout="stereo" audioRate="48k">
          <spine>
            <asset-clip ref="r2" offset="0s" name="Interview" start="3602s" duration="120120/24000s" tcFormat="NDF">
              <adjust-transform position="10 0" scale="1.2 1.2" rotation="90"/>
              <title ref="r4" lane="1" offset="3603s" name="Lower third" start="0s" duration="48048/24000s">
                <text><text-style ref="ts1">Ada Lovelace</text-style></text>
                <text-style-def id="ts1"><text-style font="Helvetica" fontSize="63" fontFace="Bold" fontColor="1 0 0 1" bold="1" alignment="left"/></text-style-def>
              </title>
              <asset-clip ref="r3" lane="-1" offset="3602s" name="Music" start="0s" duration="10s" audioRole="music">
                <adjust-volume amount="-6dB"/>
              </asset-clip>
              <marker start="3604s" duration="1001/24000s" value="Good line"/>
            </asset-clip>
            <transition name="Cross Dissolve" offset="108108/24000s" duration="24024/24000s"><filter-video ref="r5" name="Cross Dissolve"/></transition>
            <asset-clip ref="r2" offset="120120/24000s" name="Interview 2" start="3610s" duration="120120/24000s">
              <timeMap><timept time="3610s" value="3610s" interp="linear"/><timept time="3615s" value="3620s" interp="linear"/></timeMap>
            </asset-clip>
          </spine>
        </sequence>
      </project>
    </event>
  </library>
</fcpxml>"#;
    let file = dir.path().join("cut.fcpxml");
    std::fs::write(&file, doc).unwrap();
    let imp = import(&file, None).unwrap();
    let p = &imp.project;
    assert!((p.settings.fps - 24000.0 / 1001.0).abs() < 1e-9);
    let v = media_clips(p, TrackKind::Video);
    assert_eq!(v.len(), 2, "{v:#?}");
    assert_eq!(v[0].in_point, 2.0);
    assert!((v[0].transform.x - 108.0).abs() < 1e-9 && (v[0].transform.scale - 1.2).abs() < 1e-9 && v[0].transform.rotation == -90.0);
    assert!((v[1].start - 5.005).abs() < 1e-9);
    assert!((v[1].speed - 2.0).abs() < 1e-9 && (v[1].in_point - 10.0).abs() < 1e-9, "{:?}", v[1]);
    assert_eq!(v[1].transition.as_ref().map(|t| t.kind), Some(TransitionKind::Dissolve));
    let title = p.clips().find(|(_, c)| matches!(c.content, ClipContent::Text { .. })).unwrap().1;
    assert_eq!(title.start, 1.0);
    let ClipContent::Text { style } = &title.content else { unreachable!() };
    assert_eq!((style.content.as_str(), style.color.as_str(), style.font_weight), ("Ada Lovelace", "#ff0000", 700));
    let music = media_clips(p, TrackKind::Audio);
    assert_eq!(music.len(), 1);
    assert!((music[0].volume - 0.501).abs() < 0.01);
    assert!(p.tracks.iter().any(|t| t.name == "Music"));
    assert_eq!(p.markers[0].time, 2.0);
}

#[test]
fn reads_resolve_otio() {
    let dir = tempfile::tempdir().unwrap();
    let doc = r#"{"OTIO_SCHEMA": "Timeline.1", "name": "Timeline 1",
      "global_start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 86400.0},
      "tracks": {"OTIO_SCHEMA": "Stack.1", "children": [
        {"OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "Video 1", "children": [
          {"OTIO_SCHEMA": "Gap.1", "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 0}, "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 24}}},
          {"OTIO_SCHEMA": "Clip.1", "name": "shot", "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 48}, "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 48}},
           "media_reference": {"OTIO_SCHEMA": "ExternalReference.1", "target_url": "/media/shot.mov"},
           "effects": [{"OTIO_SCHEMA": "Effect.1", "effect_name": "Resolve Color"}],
           "markers": [{"OTIO_SCHEMA": "Marker.2", "name": "m", "color": "BLUE", "marked_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 72}, "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 0}}}]},
          {"OTIO_SCHEMA": "Transition.1", "transition_type": "SMPTE_Dissolve", "in_offset": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 6}, "out_offset": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 6}},
          {"OTIO_SCHEMA": "Clip.2", "name": "missing", "source_range": {"OTIO_SCHEMA": "TimeRange.1", "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 0}, "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 24}},
           "media_references": {"DEFAULT_MEDIA": {"OTIO_SCHEMA": "MissingReference.1", "name": "lost.mov"}}, "active_media_reference_key": "DEFAULT_MEDIA"}
        ]}
      ]}}"#;
    let file = dir.path().join("t.otio");
    std::fs::write(&file, doc).unwrap();
    let imp = import(&file, None).unwrap();
    let v = media_clips(&imp.project, TrackKind::Video);
    assert_eq!(imp.project.settings.fps, 24.0);
    assert_eq!((v[0].start, v[0].duration, v[0].in_point), (1.0, 2.0, 2.0));
    assert_eq!(v[1].transition.as_ref().unwrap().duration, 0.5);
    assert_eq!(imp.project.markers[0].time, 2.0);
    assert!(imp.report.dropped.iter().any(|d| d.contains("Resolve Color")));
    assert_eq!(imp.report.missing_media.len(), 2);
}

#[test]
fn bad_input_gives_errors_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    let junk: &[(&str, &[u8])] = &[
        ("a.otio", b"{"),
        ("b.otio", b"{\"OTIO_SCHEMA\": \"Timeline.1\", \"tracks\": {\"children\": [{\"OTIO_SCHEMA\": \"Track.1\", \"children\": [{\"OTIO_SCHEMA\": \"Clip.1\", \"source_range\": 5}]}]}}"),
        ("c.fcpxml", b"<fcpxml><library/></fcpxml>"),
        ("d.xml", b"<xmeml><sequence/></xmeml>"),
        ("e.xml", b"<html></html>"),
        ("f.edl", b"TITLE: x\n001 AX V C 99:99:99:99"),
        ("g.otioz", b"PK\x03\x04garbage"),
        ("h.fcpxml", b"<fcpxml><library><event><project><sequence><spine><asset-clip ref=\"nope\" duration=\"x\"/></spine></sequence></project></event></library></fcpxml>"),
    ];
    for (name, bytes) in junk {
        let f = dir.path().join(name);
        std::fs::write(&f, bytes).unwrap();
        let _ = import(&f, None);
    }
    assert!(import(&dir.path().join("a.otio"), None).is_err());
    assert!(import(&dir.path().join("c.fcpxml"), None).is_err());
    assert!(import(&dir.path().join("nothing.otio"), None).is_err());
    assert!(format("fcpxm").unwrap_err().contains("fcpxml"));
}

#[test]
fn every_format_has_a_reader_or_says_so() {
    for f in FORMATS {
        let ext = f.extensions[0];
        if f.export != Support::No {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join(format!("x.{ext}"));
            export(&sample(dir.path()), f.id, &file).unwrap_or_else(|e| panic!("{}: {e}", f.id));
        }
    }
}
