use std::sync::Arc;

use serde_json::{Value, json};

use crate::registry::{self, Args, Ctx};
use crate::session::{Session, SessionOptions, Source};

fn session(dir: &std::path::Path) -> Arc<Session> {
    Session::new(SessionOptions {
        data_dir: Some(dir.join("data")),
        config_dir: Some(dir.join("config")),
        secrets: None,
        headless: true,
    })
    .unwrap()
}

#[tokio::test]
async fn window_project_guards_reject_stale_reads_and_writes_without_touching_the_new_document() {
    let dir=tempfile::tempdir().unwrap();let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Original"})).await;
    let original=s.project().unwrap();
    let mut copy=original.clone();copy.id=kimchi_core::Id::new_v4();copy.name="Copy".into();
    s.guard_project(Some(original.id),async {
        assert_eq!(s.project().unwrap(),original);
        s.open_doc(copy.clone(),crate::Location::Library);
        tokio::task::yield_now().await;
        assert!(s.project().unwrap_err().contains("project changed"));
        // A command may have read its inputs already; its eventual write is checked too.
        let edit=kimchi_core::Edit::RenameProject {name:"Stale edit".into()};
        assert!(s.apply("test",Source::Window,&edit,None).unwrap_err().contains("project changed"));
        let err=crate::call(&s,Source::Window,"clip.addSolid",json!({"color":"#fff","duration":1})).await.unwrap_err();
        assert!(err.contains("project changed"),"{err}");
    }).await;
    assert_eq!(s.project().unwrap(),copy);
    assert_eq!(ok(&s,Source::Cli,"history.list",json!({})).await["undo"],json!([]));
    let edit=kimchi_core::Edit::RenameProject {name:"Current edit".into()};
    s.guard_project(Some(copy.id),async {s.apply("test",Source::Window,&edit,None).unwrap();}).await;
    assert_eq!(s.project().unwrap().name,"Current edit");
    s.guard_project(None,async {assert!(s.apply("test",Source::Window,&edit,None).is_err());}).await;
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(s.project().unwrap(),copy,"the guard expires with its command");
}

#[tokio::test(flavor = "multi_thread")]
async fn generated_audio_lands_on_an_audio_track_with_measured_duration_and_provenance() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};
    if kimchi_media::Tools::locate().is_err() { return; }
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let server = MockServer::start().await;
    // A real one-second mono PCM WAV, so the same probe/import path as production runs.
    let samples = 8000u32;
    let data_size = samples * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF"); wav.extend_from_slice(&(36 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt "); wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&samples.to_le_bytes()); wav.extend_from_slice(&(samples * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes()); wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data"); wav.extend_from_slice(&data_size.to_le_bytes());
    wav.resize(44 + data_size as usize, 0);
    Mock::given(method("POST")).and(path("/v2beta/audio/stable-audio-2/text-to-audio"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(wav)).expect(2).mount(&server).await;
    s.harness.set_key("stability", Some("test-key")).unwrap();
    s.harness.set_settings("stability", kimchi_gen::ProviderSettings { enabled: true, base_url: Some(server.uri()), ..Default::default() });
    ok(&s, Source::Window, "project.create", json!({"name":"Generated sound"})).await;
    let result = ok(&s, Source::Window, "generate.submit", json!({
        "provider":"stability", "model":"stable-audio-2.5", "task":"text_to_audio", "prompt":"Quiet ambience",
        "duration":5, "start":2, "wait":true
    })).await;
    assert_eq!(result["status"], "succeeded", "{result}");
    let p = s.project().unwrap();
    let (track, clip) = p.clips().next().expect("audio clip on the timeline");
    assert_eq!(track.kind, kimchi_core::TrackKind::Audio);
    assert_eq!(clip.start, 2.);
    assert!((clip.duration - 1.).abs() < 0.01, "duration is measured from the generated audio");
    let asset = p.asset(clip.asset_id().unwrap()).unwrap();
    assert_eq!(asset.kind, kimchi_core::MediaKind::Audio);
    let kimchi_core::AssetOrigin::Generated(origin) = &asset.origin else { panic!("generation provenance") };
    assert_eq!(origin.provider, "stability");
    assert_eq!(origin.prompt, "Quiet ambience");
    assert_eq!(origin.params["task"], "text_to_audio");
    // Regenerate from the media item (the window's media menu): the same request, into the library.
    let again = ok(&s, Source::Window, "generate.regenerate", json!({ "assetId": asset.id, "variation": true, "wait": true })).await;
    assert_eq!(again["status"], "succeeded", "{again}");
    // A library result has no placeholder for `wait` to watch: give it a moment to land.
    let mut p = s.project().unwrap();
    for _ in 0..100 {
        if p.assets.len() > 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        p = s.project().unwrap();
    }
    assert_eq!(p.clips().count(), 1, "a media item's regeneration lands in the library only");
    let new = p.assets.iter().find(|a| a.id != asset.id).expect("the regenerated media item");
    let kimchi_core::AssetOrigin::Generated(again) = &new.origin else { panic!("generation provenance") };
    assert_eq!((again.prompt.as_str(), &again.params["task"], &again.params["duration"]), ("Quiet ambience", &json!("text_to_audio"), &origin.params["duration"]));
    let err = registry::call(&s, Source::Window, "generate.regenerate", json!({})).await.unwrap_err();
    assert!(err.contains("clipId") && err.contains("assetId"), "{err}");
}

async fn ok(s: &Arc<Session>, source: Source, name: &str, params: Value) -> Value {
    registry::call(s, source, name, params).await.unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn mesh_operations_preserve_existing_precision_and_tiny_offsets() {
    let dir=tempfile::tempdir().unwrap();let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Mesh operation precision"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"panel","type":"mesh","vertices":[[1e-12,0,0],[1,0,0],[1,0,1],[1e-12,0,1],
            [3.123456789123,0,0],[4,0,0],[4,0,1],[3.123456789123,0,1]],"faces":[[3,2,1,0],[7,6,5,4]]}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let result=ok(&s,Source::Window,"motion.editMesh",json!({"clipId":clip,"id":"panel","op":"extrude","faces":[0],"params":{"distance":0}})).await;
    let flat=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let original=before["scene"]["objects"][0]["vertices"].as_array().unwrap();
    let vertices=flat["scene"]["objects"][0]["vertices"].as_array().unwrap();
    assert_eq!(vertices.len(),12);assert_eq!(&vertices[..8],original);
    ok(&s,Source::Window,"motion.editMesh",json!({"clipId":clip,"id":"panel","op":"translate",
        "vertices":result["result"]["selection"]["vertices"],"params":{"offset":[0,1e-12,0]}})).await;
    let pulled=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let vertices=pulled["scene"]["objects"][0]["vertices"].as_array().unwrap();
    assert_eq!(&vertices[..8],original);
    for p in &vertices[8..] {assert_eq!(p[1],json!(1e-12));}
    ok(&s,Source::Window,"history.undo",json!({})).await;
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
}

#[tokio::test(flavor = "multi_thread")]
async fn exact_mesh_vertex_updates_preserve_unselected_geometry_and_reject_invalid_batches() {
    let dir=tempfile::tempdir().unwrap();let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Exact mesh editing"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"panel","type":"mesh","vertices":[[0,0,0],[1,0,0],[1,1,0],[0,1,0]],"faces":[[0,1,2,3]],
         "uvs":[[[0,0],[1,0],[1,1],[0,1]]],"rotation":[10,20,30],"keyframes":{"position.x":[[0,0],[2,2]]}}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let history=ok(&s,Source::Cli,"history.list",json!({})).await;
    let update=|vertices:Value|json!({"clipId":clip,"id":"panel","vertices":vertices,"expectedVertexCount":4,"coalesce":"gesture:exact-vertices"});
    for amount in [0.25,1e-12] {
        ok(&s,Source::Window,"motion.updateMeshVertices",update(json!([{"index":0,"position":[amount,2,3]},{"index":2,"position":[2,3,4]}]))).await;
    }
    let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let mut expected=before.clone();
    expected["scene"]["objects"][0]["vertices"][0]=json!([1e-12,2.,3.]);expected["scene"]["objects"][0]["vertices"][2]=json!([2.,3.,4.]);
    assert_eq!(after,expected);
    assert_eq!(ok(&s,Source::Cli,"history.list",json!({})).await["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    let changed=s.project().unwrap();let steps=ok(&s,Source::Cli,"history.list",json!({})).await;
    for vertices in [json!([]),json!([{"index":0,"position":[9,9,9]},{"index":20,"position":[1,2,3]}]),
        json!([{"index":0,"position":[9,9,9]},{"index":0,"position":[1,2,3]}]),json!([{"index":0,"position":[1,2]}]),
        json!([{"index":-1,"position":[1,2,3]}]),json!([{"index":0,"position":[1,null,3]}]),json!([{"index":0,"position":[1,2,3],"typo":4}])] {
        assert!(registry::call(&s,Source::Window,"motion.updateMeshVertices",update(vertices)).await.is_err());
        assert_eq!(s.project().unwrap(),changed);assert_eq!(ok(&s,Source::Cli,"history.list",json!({})).await,steps);
    }
    let mut stale=update(json!([{"index":0,"position":[9,9,9]}]));stale["expectedVertexCount"]=json!(3);
    assert!(registry::call(&s,Source::Window,"motion.updateMeshVertices",stale).await.unwrap_err().contains("start the transform again"));
    assert_eq!(s.project().unwrap(),changed);
    ok(&s,Source::Window,"history.undo",json!({})).await;assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    ok(&s,Source::Window,"motion.updateMeshVertices",update(json!([{"index":1,"position":[7,8,9]}]))).await;
    ok(&s,Source::Window,"motion.updateMeshVertices",update(json!([{"index":0,"position":[1,2,3]}]))).await;
    let final_scene=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    assert_eq!(final_scene["scene"]["objects"][0]["vertices"][1],json!([7.,8.,9.]),"a preview never replaces an unselected vertex");
}

#[tokio::test(flavor = "multi_thread")]
async fn overflowing_mesh_edits_preserve_the_scene_and_undo_history() {
    let dir=tempfile::tempdir().unwrap();let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Mesh precision"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"panel","type":"mesh","vertices":[[-2,0,0],[2,0,0],[2,1,0],[-2,1,0]],"faces":[[0,1,2,3]]}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    let original=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    // Storing a finite coordinate must not overflow just to round its decimal places.
    ok(&s,Source::Window,"motion.editMesh",json!({"clipId":clip,"id":"panel","op":"scale","select":{"all":true},
        "params":{"factor":[1e300,1,1],"pivot":[0,0,0]}})).await;
    let large=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    assert_eq!(large["scene"]["objects"][0]["vertices"][0][0],-2e300);
    kimchi_core::Scene::from_json(&large["scene"]).expect("stored mesh remains readable");
    let before=serde_json::to_value(s.project().unwrap()).unwrap();
    let history=ok(&s,Source::Cli,"history.list",json!({})).await;
    let error=registry::call(&s,Source::Window,"motion.editMesh",json!({"clipId":clip,"id":"panel","op":"scale",
        "select":{"all":true},"params":{"factor":[1e308,1,1],"pivot":[0,0,0]}})).await.unwrap_err();
    assert!(error.contains("numeric range"),"{error}");
    assert_eq!(serde_json::to_value(s.project().unwrap()).unwrap(),before,"overflow must not erase faces or store null vertices");
    assert_eq!(ok(&s,Source::Cli,"history.list",json!({})).await,history);
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,original);
}

#[tokio::test(flavor = "multi_thread")]
async fn arrange_objects_respects_parents_animation_time_and_undo() {
    use kimchi_core::{Scene, motion::find_object};
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Arrange"})).await;
    let added = ok(&s, Source::Window, "motion.add", json!({"start":5,"duration":4,"scene":{"type":"3d","objects":[
        {"id":"parent","type":"group","position":[10,0,0],"rotation":[0,0,90],"scale":[2,3,1],"children":[
            {"id":"child","type":"box","position":[1,1,0],"keyframes":{
                "position":[[0,[1,1,0]],[4,[1,1,0]]], "position.y":[[0,1],[4,1]], "y":[[0,1],[4,1]]
            }}
        ]}, {"id":"anchor","type":"box","position":[2,4,0]}
    ]}})).await;
    let clip = added["clips"][0]["id"].clone();
    ok(&s, Source::Window, "clip.update", json!({"clipId":clip,"speed":2})).await;
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await["scene"].clone();
    let result = ok(&s, Source::Window, "motion.arrangeObjects", json!({"clipId":clip,"ids":["child","anchor"],"operation":"alignActive","axis":"x","time":6})).await;
    assert_eq!(result["moved"], json!(["child"]));
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await["scene"].clone();
    let Scene::Space(scene) = Scene::from_json(&after).unwrap() else { panic!() };
    let child = find_object(&scene.objects, "child").unwrap();
    for prop in ["position", "position.y", "y"] {
        assert_eq!(child.keyframes[prop].len(), 3, "preserve and key both vector and component channels");
        assert_eq!(child.keyframes[prop][1].time, 2., "timeline 6 maps to scene 2");
    }
    let world = kimchi_media::render::space::viewport::world_matrix(&scene, 2., "child").unwrap();
    assert!((world[3][0] - 2.).abs() < 1e-5 && (world[3][1] - 2.).abs() < 1e-5, "world X aligns, Y stays unchanged: {world:?}");
    let at_start = child.at(0.).position.0;
    assert_eq!(at_start, [1.,1.,0.], "the original animation endpoints stay intact");
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await["scene"], before);
}

#[tokio::test(flavor = "multi_thread")]
async fn arrange_objects_distributes_roots_and_rejects_unsolvable_edits_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Arrange roots"})).await;
    let added = ok(&s, Source::Window, "motion.add", json!({"scene":{"type":"3d","objects":[
        {"id":"a","type":"group","position":[-4,0,0],"children":[{"id":"child","type":"box","position":[2,0,0]}]},
        {"id":"b","type":"box","position":[-3,1,0]}, {"id":"c","type":"box","position":[0,2,0]},
        {"id":"d","type":"box","position":[8,3,0]}
    ]}})).await;
    let clip = added["clips"][0]["id"].clone();
    let args = json!({"clipId":clip,"ids":["d","c","child","a","b"],"operation":"distribute","axis":"x"});
    let result = ok(&s, Source::Window, "motion.arrangeObjects", args.clone()).await;
    assert_eq!(result["moved"], json!(["b","c"]));
    for (id, x, y) in [("a",-4.,0.),("b",0.,1.),("c",4.,2.),("d",8.,3.),("child",2.,0.)] {
        let object = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip,"id":id})).await;
        assert_eq!(object["position"], json!([x,y,0.]));
    }
    let history = ok(&s, Source::Cli, "history.list", json!({})).await;
    assert_eq!(ok(&s, Source::Window, "motion.arrangeObjects", args).await["changed"], false);
    assert_eq!(ok(&s, Source::Cli, "history.list", json!({})).await, history, "no empty undo step");
    ok(&s, Source::Window, "motion.updateLayer", json!({"clipId":clip,"id":"c","props":{"expressions":{"position.x":"4"}}})).await;
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let error = registry::call(&s, Source::Window, "motion.arrangeObjects", json!({"clipId":clip,"ids":["b","c","d"],"operation":"alignActive","axis":"x"})).await.unwrap_err();
    assert!(error.contains("expression"), "{error}");
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before, "no partial edit of b");
}

#[tokio::test(flavor = "multi_thread")]
async fn studio_multi_object_updates_are_atomic_and_coalesce_as_one_undo_step() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Transform objects"})).await;
    let objects: Vec<_> = (0..20).map(|i| json!({"id":format!("box{i}"),"type":"box"})).collect();
    let added = ok(&s, Source::Window, "motion.add", json!({"scene":{"type":"3d","objects":objects}})).await;
    let clip = added["clips"][0]["id"].clone();
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    for x in [1.,2.,3.] {
        let updates: Vec<_> = (0..20).map(|i| json!({"id":format!("box{i}"),"props":{"position.x":x}})).collect();
        ok(&s, Source::Window, "motion.updateLayers", json!({"clipId":clip,"updates":updates,"coalesce":"one-transform"})).await;
    }
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    assert!(after["scene"]["objects"].as_array().unwrap().iter().all(|o| o["position"][0] == 3.));
    assert!(registry::call(&s, Source::Window, "motion.updateLayers", json!({"clipId":clip,"updates":[
        {"id":"box0","props":{"position.x":100}}, {"id":"missing","props":{"position.x":100}}
    ]})).await.is_err());
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, after);
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn shifting_keyframes_accounts_for_trim_and_clip_speed() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Trimmed animation"})).await;
    let added = ok(&s, Source::Window, "motion.add", json!({"start":5,"duration":4,"scene":{"type":"3d","objects":[
        {"id":"box","type":"box","keyframes":{"position.x":[[0,0],[1,10],[2,20],[4,40]]}}
    ]}})).await;
    let clip = added["clips"][0]["id"].clone();
    ok(&s, Source::Window, "clip.update", json!({"clipId":clip,"speed":2})).await;
    ok(&s, Source::Window, "clip.trim", json!({"clipId":clip,"edge":"start","time":5.25})).await;
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    ok(&s, Source::Window, "motion.shiftKeyframes", json!({"clipId":clip,"id":"box","property":"position.x","times":[5.5],"by":0.25})).await;
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let scene = kimchi_core::Scene::from_json(&after["scene"]).unwrap();
    let kimchi_core::Scene::Space(scene) = scene else { panic!() };
    let keys = &scene.objects[0].keyframes["position.x"];
    assert_eq!(keys.iter().map(|k| k.time).collect::<Vec<_>>(), [0.,1.5,2.,4.]);
    assert_eq!(keys[1].value.as_f64(), Some(10.));
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn shifting_keyframes_preserves_group_spacing_and_checks_every_time() {
    let dir=tempfile::tempdir().unwrap();
    let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Group retiming"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"start":10,"duration":5,"scene":{"objects":[
        {"id":"box","type":"box","keyframes":{"position.x":[[1,10],[2,20,"hold"],[4,40,"easeOut"]],"position.y":[[0.5,5],[1.5,15]]}}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    ok(&s,Source::Window,"clip.update",json!({"clipId":clip,"speed":2})).await;
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let result=ok(&s,Source::Window,"motion.shiftKeyframes",json!({"clipId":clip,"id":"box","by":-10})).await;
    assert_eq!(result["moved"],5);
    assert_eq!(result["by"],-0.25,"the applied timeline delta reflects the whole group's clamp");
    let shifted=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip,"id":"box"})).await;
    let keys:kimchi_core::Keyframes=serde_json::from_value(shifted["keyframes"].clone()).unwrap();
    assert_eq!(keys["position.x"].iter().map(|k| k.time).collect::<Vec<_>>(),[0.5,1.5,3.5]);
    assert_eq!(keys["position.y"].iter().map(|k| k.time).collect::<Vec<_>>(),[0.,1.]);
    assert_eq!(keys["position.x"][1].easing,kimchi_core::Easing::Hold);
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    for times in [json!([10.5,"bad"]),json!([]),json!([null])] {
        assert!(registry::call(&s,Source::Window,"motion.shiftKeyframes",json!({"clipId":clip,"id":"box","by":1,"times":times})).await.is_err());
    }
    assert!(registry::call(&s,Source::Window,"motion.shiftKeyframes",json!({"clipId":clip,"id":"box","by":f64::MAX})).await.is_err());
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before,"invalid times leave every channel intact");
    ok(&s,Source::Window,"motion.shiftKeyframes",json!({"clipId":clip,"id":"box","property":"position.x","times":[11,12],"by":-0.5})).await;
    let item=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip,"id":"box"})).await;
    let keys:kimchi_core::Keyframes=serde_json::from_value(item["keyframes"].clone()).unwrap();
    assert_eq!(keys["position.x"].iter().map(|k| (k.time,k.value.as_f64().unwrap())).collect::<Vec<_>>(),[(1.,20.),(3.,40.)]);
    assert_eq!(keys["position.y"].iter().map(|k| k.time).collect::<Vec<_>>(),[0.5,1.5]);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicating_keyframes_preserves_sources_and_rejects_conflicts_atomically() {
    let dir=tempfile::tempdir().unwrap();
    let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Key copies"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"start":5,"duration":5,"scene":{"objects":[
        {"id":"box","type":"box","position":[9,8,7],"keyframes":{"position.x":[[0,0],[1,10,"hold"],[2,20,"easeOut"]],"position.y":[[1,5],[3,15]]}}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    ok(&s,Source::Window,"clip.update",json!({"clipId":clip,"speed":2})).await;
    ok(&s,Source::Window,"clip.trim",json!({"clipId":clip,"edge":"start","time":5.25})).await;
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let selected=json!([{"id":"box","property":"position.x","time":5.5},{"id":"box","property":"position.x","time":6},{"id":"box","property":"position.y","time":5.5}]);
    let copied=ok(&s,Source::Window,"motion.duplicateKeyframes",json!({"clipId":clip,"keys":selected,"by":2})).await;
    assert_eq!(copied["copied"],3);
    assert_eq!(copied["selectedKeys"],json!([{"id":"box","property":"position.x","time":5.0},{"id":"box","property":"position.x","time":6.0},{"id":"box","property":"position.y","time":5.0}]));
    let item=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip,"id":"box"})).await;
    let keys:kimchi_core::Keyframes=serde_json::from_value(item["keyframes"].clone()).unwrap();
    let x=&keys["position.x"];
    assert_eq!(x.iter().map(|k| (k.time,k.value.as_f64().unwrap())).collect::<Vec<_>>(),[(0.,0.),(1.,10.),(2.,20.),(5.,10.),(6.,20.)]);
    assert_eq!(x[3].easing,x[1].easing);
    assert_eq!(x[4].easing,x[2].easing);
    assert_eq!(item["position"],json!([9.,8.,7.]));
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    for params in [
        json!({"keys":selected,"by":0}),json!({"keys":selected,"by":0.5}),
        json!({"keys":[{"id":"box","property":"position.x","time":5.5},{"id":"missing","property":"x","time":5}],"by":2}),
        json!({"keys":[{"id":"box","property":"position.x","time":5.5},{"id":"box","property":"position.x","time":5.5}],"by":2}),
        json!({"keys":[{"id":"box","property":"position.x","time":5.5,"value":55}],"by":2}),
        json!({"keys":selected,"by":f64::MAX}),json!({"keys":[],"by":2})
    ] {
        let mut params=params;
        params["clipId"]=clip.clone();
        assert!(registry::call(&s,Source::Window,"motion.duplicateKeyframes",params).await.is_err());
        assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    }
    ok(&s,Source::Window,"motion.duplicateKeyframes",json!({"clipId":clip,"keys":[{"id":"box","property":"position.x","time":5.5},{"id":"box","property":"position.x","time":6}],"by":0.5,"replace":true})).await;
    let item=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip,"id":"box"})).await;
    let keys:kimchi_core::Keyframes=serde_json::from_value(item["keyframes"].clone()).unwrap();
    assert_eq!(keys["position.x"].iter().map(|k| (k.time,k.value.as_f64().unwrap())).collect::<Vec<_>>(),[(0.,0.),(1.,10.),(2.,10.),(3.,20.)],"overlapping copies read original values before replacing destinations");
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicating_selections_remaps_links_and_undoes_as_one_edit() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Copied rigs"})).await;
    let added = ok(&s, Source::Window, "motion.add", json!({"scene":{"type":"3d",
        "objects":[
            {"id":"root","type":"box","children":[{"id":"child","type":"sphere","expressions":{"position.x":"prop('root', 'position.x') + 1"}}]},
            {"id":"follower","type":"box","constraints":[{"id":"child","type":"lookAt","target":"child"}],
                "modifiers":[{"type":"boolean","object":"child"}],"expressions":{"position.x":"prop('child', 'position.x') + prop('outside', 'position.x')"}},
            {"id":"outside","type":"box"},{"id":"child2","type":"box"}],
        "lights":[{"id":"lamp","type":"point","constraints":[{"type":"lookAt","target":"child"}]}],
        "cameras":[{"id":"side","constraints":[{"type":"lookAt","target":"root"}]}],"activeCamera":"side"
    }})).await;
    let clip = added["clips"][0]["id"].clone();
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let copied = ok(&s, Source::Window, "motion.duplicateLayers", json!({"clipId":clip,"ids":["child","root","follower","root","lamp","side"]})).await;
    assert_eq!(copied["ids"], json!(["root2","follower2","lamp2","side2"]));
    assert_eq!(copied["idMap"]["child"], "child3", "ids also avoid unselected objects");
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let scene = &after["scene"];
    let objects = scene["objects"].as_array().unwrap();
    assert_eq!(objects.iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(), ["root","root2","follower","follower2","outside","child2"]);
    assert_eq!(objects[1]["children"].as_array().unwrap().len(), 1);
    assert_eq!(objects[1]["children"][0]["id"], "child3");
    assert_eq!(objects[1]["children"][0]["expressions"]["position.x"], "prop('root2', 'position.x') + 1");
    assert_eq!(objects[3]["constraints"][0]["target"], "child3");
    assert_eq!(objects[3]["constraints"][0]["id"], "child", "stack ids name animation channels, not scene objects");
    assert_eq!(objects[3]["modifiers"][0]["object"], "child3");
    assert_eq!(objects[3]["expressions"]["position.x"], "prop('child3', 'position.x') + prop('outside', 'position.x')");
    assert_eq!(scene["lights"][1]["constraints"][0]["target"], "child3");
    assert_eq!(scene["cameras"][1]["constraints"][0]["target"], "root2");
    assert_eq!(scene["activeCamera"], "side");
    assert_eq!(objects[0], before["scene"]["objects"][0]);
    assert_eq!(objects[2], before["scene"]["objects"][1]);
    assert!(registry::call(&s, Source::Window, "motion.duplicateLayers", json!({"clipId":clip,"ids":["root","missing"]})).await.is_err());
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, after);
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
    // The single-copy command shares the hierarchy and light/camera fixes.
    let copy = ok(&s, Source::Window, "motion.duplicateLayer", json!({"clipId":clip,"id":"root","newId":"rig"})).await;
    assert_eq!(copy["id"], "rig");
    let rig = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip,"id":"rig"})).await;
    assert_eq!(rig["children"][0]["expressions"]["position.x"], "prop('rig', 'position.x') + 1");
    assert_eq!(ok(&s, Source::Window, "motion.duplicateLayer", json!({"clipId":clip,"id":"lamp"})).await["id"], "lamp2");
    assert_eq!(ok(&s, Source::Window, "motion.duplicateLayer", json!({"clipId":clip,"id":"side"})).await["id"], "side2");
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicating_layer_groups_keeps_mattes_parents_and_compositions() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Copied layers"})).await;
    let added = ok(&s, Source::Window, "motion.add", json!({"scene":{
        "layers":[{"id":"group","type":"group","layers":[
            {"id":"a","type":"rect"},
            {"id":"b","type":"text","text":"a","parent":"a","matte":{"layer":"a"},"expressions":{"x":"prop('a', 'x') + 2"}}
        ]}],
        "compositions":[{"id":"group2","layers":[{"id":"inside","type":"rect"}]}]
    }})).await;
    let clip = added["clips"][0]["id"].clone();
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let copied = ok(&s, Source::Window, "motion.duplicateLayers", json!({"clipId":clip,"ids":["b","group","inside"]})).await;
    assert_eq!(copied["ids"], json!(["group3","inside2"]));
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let b = &after["scene"]["layers"][1]["layers"][1];
    assert_eq!(b["id"], "b2");
    assert_eq!(b["parent"], "a2");
    assert_eq!(b["matte"]["layer"], "a2");
    assert_eq!(b["expressions"]["x"], "prop('a2', 'x') + 2");
    assert_eq!(b["text"], "a", "visible text is not a reference");
    assert_eq!(after["scene"]["compositions"][0]["layers"][1]["id"], "inside2");
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_linked_selections_validates_the_remaining_scene_atomically() {
    let scenes = [json!({"layers":[
        {"id":"root","type":"group","layers":[{"id":"target","type":"rect"}]},
        {"id":"follower","type":"ellipse","parent":"root","matte":{"layer":"root"}},
        {"id":"outside","type":"rect"}
    ]}), json!({"type":"3d","objects":[
        {"id":"root","type":"box","children":[{"id":"target","type":"sphere"}]},
        {"id":"follower","type":"box","constraints":[{"type":"lookAt","target":"target"}]},
        {"id":"outside","type":"box"}
    ]})];
    for scene in scenes {
        let dir = tempfile::tempdir().unwrap();
        let s = session(dir.path());
        ok(&s, Source::Window, "project.create", json!({"name":"Remove selection"})).await;
        let added = ok(&s, Source::Window, "motion.add", json!({"scene":scene})).await;
        let clip = added["clips"][0]["id"].clone();
        let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
        for ids in [json!(["root"]),json!(["outside","missing"]),json!([])] {
            assert!(registry::call(&s, Source::Window, "motion.removeLayers", json!({"clipId":clip,"ids":ids})).await.is_err());
            assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
        }
        ok(&s, Source::Window, "motion.removeLayers", json!({"clipId":clip,"ids":["root","target","follower","root"]})).await;
        let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
        assert_eq!(kimchi_core::Scene::from_json(&after["scene"]).unwrap().ids(), ["outside"]);
        ok(&s, Source::Window, "history.undo", json!({})).await;
        assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_selected_roots_keeps_scene_order_children_and_one_undo_step() {
    let dir=tempfile::tempdir().unwrap();
    let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Move selection"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"scene":{"type":"3d","objects":[
        {"id":"a","type":"group","position":[2,3,4],"children":[{"id":"child","type":"box","keyframes":{"rotation.y":[[0,0],[1,90,"easeOutBack"]]}}]},
        {"id":"b","type":"box"},{"id":"c","type":"sphere"},{"id":"d","type":"group","children":[{"id":"inside","type":"box"}]}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let moved=ok(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["c","child","a","c"],"index":1})).await;
    assert_eq!(moved["moved"],json!(["a","c"]));
    let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let objects=after["scene"]["objects"].as_array().unwrap();
    assert_eq!(objects.iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(),["b","a","c","d"]);
    assert_eq!(objects[1],before["scene"]["objects"][0],"children, animation and local transforms stay intact");
    let history=ok(&s,Source::Cli,"history.list",json!({})).await;
    assert_eq!(ok(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["a","c"],"index":1})).await["changed"],false);
    assert_eq!(ok(&s,Source::Cli,"history.list",json!({})).await,history,"no empty undo entry");
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    ok(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["c","child","a"],"parent":"d","index":0})).await;
    let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let children=after["scene"]["objects"][1]["children"].as_array().unwrap();
    assert_eq!(children.iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(),["a","c","inside"]);
    assert_eq!(children[0],before["scene"]["objects"][0]);
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    let history=ok(&s,Source::Cli,"history.list",json!({})).await;
    for args in [json!({"ids":[]}),json!({"ids":["a",7]}),json!({"ids":["b","missing"]}),
        json!({"ids":["b","camera"]}),json!({"ids":["a","b"],"parent":"child"}),
        json!({"ids":["a","b"],"parent":"a"}),json!({"ids":["a","b"],"parent":"missing"}),
        json!({"ids":["b","inside"]}),json!({"ids":["a","b"],"index":-1})] {
        let mut args=args;args["clipId"]=clip.clone();
        assert!(registry::call(&s,Source::Window,"motion.moveLayers",args.clone()).await.is_err(),"{args}");
        assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
        assert_eq!(ok(&s,Source::Cli,"history.list",json!({})).await,history);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_selected_layers_handles_compositions_and_reversed_display_order() {
    let dir=tempfile::tempdir().unwrap();
    let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Move layers"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"scene":{"layers":[
        {"id":"a","type":"rect"},{"id":"b","type":"ellipse","parent":"a","matte":{"layer":"a"}},
        {"id":"group","type":"group","layers":[]},{"id":"card","type":"comp","comp":"card"}
    ],"compositions":[{"id":"card","layers":[{"id":"inside","type":"text","text":"kept"}]}]}})).await;
    let clip=added["clips"][0]["id"].clone();
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    ok(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["b","a"],"parent":"group"})).await;
    let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    assert_eq!(after["scene"]["layers"][0]["layers"],json!([before["scene"]["layers"][0],before["scene"]["layers"][1]]));
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    let moved=ok(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["inside","card"],"parent":"","index":0})).await;
    assert_eq!(moved["moved"],json!(["card","inside"]),"composition contents are not structural children of a same-named comp layer");
    let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    assert_eq!(after["scene"]["layers"][0]["id"],"card");
    assert_eq!(after["scene"]["layers"][1]["id"],"inside");
    assert_eq!(after["scene"]["compositions"][0]["layers"],json!([]));
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    ok(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["b","a"],"parent":"card","index":0})).await;
    let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    let layers=after["scene"]["compositions"][0]["layers"].as_array().unwrap();
    assert_eq!(layers.iter().map(|o| o["id"].as_str().unwrap()).collect::<Vec<_>>(),["a","b","inside"]);
    ok(&s,Source::Window,"history.undo",json!({})).await;
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_selected_layers_distinguishes_same_named_groups_and_compositions() {
    let dir=tempfile::tempdir().unwrap();
    let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Source lists"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"scene":{
        "layers":[{"id":"bucket","type":"group","layers":[{"id":"a","type":"rect"},{"id":"b","type":"ellipse"}]}],
        "compositions":[{"id":"bucket","layers":[{"id":"c","type":"rect"}]}]
    }})).await;
    let clip=added["clips"][0]["id"].clone();
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    assert!(registry::call(&s,Source::Window,"motion.moveLayers",json!({"clipId":clip,"ids":["a","c"]})).await.is_err());
    for (command,args) in [("motion.moveLayer",json!({"id":"a"})),("motion.moveLayers",json!({"ids":["a"]}))] {
        let mut args=args;args["clipId"]=clip.clone();
        ok(&s,Source::Window,command,args).await;
        let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
        assert_eq!(after["scene"]["layers"][0]["layers"][0]["id"],"b");
        assert_eq!(after["scene"]["layers"][0]["layers"][1]["id"],"a");
        assert_eq!(after["scene"]["compositions"],before["scene"]["compositions"]);
        ok(&s,Source::Window,"history.undo",json!({})).await;
        assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn animated_property_updates_keep_easing_and_can_restore_exact_channels() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Animated transforms"})).await;
    let keys = json!([[0,0],[1,2,"easeOutBack"]]);
    let added = ok(&s, Source::Window, "motion.add", json!({"scene":{"type":"3d","objects":[
        {"id":"box","type":"box","position":[10,0,0],"keyframes":{"position.x":keys,"rotation.z":[[0,0],[1,90,"easeInOut"]]}}
    ]}})).await;
    let clip = added["clips"][0]["id"].clone();
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    ok(&s, Source::Window, "motion.updateLayer", json!({"clipId":clip,"id":"box","props":{"position.x":3},"time":1})).await;
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let kimchi_core::Scene::Space(scene) = kimchi_core::Scene::from_json(&after["scene"]).unwrap() else { panic!() };
    assert_eq!(scene.objects[0].keyframes["position.x"][1].easing, kimchi_core::Easing::parse("easeOutBack").unwrap());
    ok(&s, Source::Window, "motion.updateLayer", json!({"clipId":clip,"id":"box","props":{"position.x":5},"time":0.5})).await;
    ok(&s, Source::Window, "motion.updateLayer", json!({"clipId":clip,"id":"box","props":{"position.x":10,"keyframes":{"position.x":keys}},"time":0.5})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before, "base values and original curves restored; unrelated channels stay unchanged");
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_keyframes_keeps_base_values_and_existing_easing() {
    let dir=tempfile::tempdir().unwrap();let s=session(dir.path());
    ok(&s,Source::Window,"project.create",json!({"name":"Insert keys"})).await;
    let added=ok(&s,Source::Window,"motion.add",json!({"start":5,"duration":4,"scene":{"type":"3d","objects":[
        {"id":"box","type":"box","position":[9,8,7],"keyframes":{"position.x":[[0,0],[2,4,"easeOutBack"]]}}
    ]}})).await;
    let clip=added["clips"][0]["id"].clone();
    ok(&s,Source::Window,"clip.update",json!({"clipId":clip,"speed":2})).await;
    let before=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
    for (time,value,easing) in [(5.5,None,None),(6.,Some(json!(6)),None),(6.,Some(json!(3)),Some("hold"))] {
        let mut args=json!({"clipId":clip,"id":"box","property":"position.x","time":time});
        if let Some(value)=value {args["value"]=value;}
        if let Some(easing)=easing {args["easing"]=json!(easing);}
        ok(&s,Source::Window,"motion.addKeyframe",args).await;
        let after=ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await;
        let kimchi_core::Scene::Space(scene)=kimchi_core::Scene::from_json(&after["scene"]).unwrap() else {panic!()};
        assert_eq!(scene.objects[0].position.0,[9.,8.,7.]);
        let keys=&scene.objects[0].keyframes["position.x"];
        if time==5.5 {
            assert_eq!(keys.len(),3);assert_eq!(keys[1].time,1.);
            let original=kimchi_core::Scene::from_json(&before["scene"]).unwrap();
            assert_eq!(Some(keys[1].value.clone()),original.value("box","position.x",1.));
        } else {
            assert_eq!(keys.len(),2);
            assert_eq!(keys[1].easing,kimchi_core::Easing::parse(easing.unwrap_or("easeOutBack")).unwrap());
        }
        ok(&s,Source::Window,"history.undo",json!({})).await;
        assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
    }
    assert!(registry::call(&s,Source::Window,"motion.addKeyframe",json!({"clipId":clip,"id":"box","property":"position.x","time":6,"value":"wrong type"})).await.is_err());
    assert_eq!(ok(&s,Source::Cli,"motion.get",json!({"clipId":clip})).await,before);
}

#[tokio::test(flavor = "multi_thread")]
async fn keyframe_edits_are_atomic_preserve_curves_and_support_swaps() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({"name":"Keyframe editing"})).await;
    let added = ok(&s, Source::Window, "motion.add", json!({"start":5,"duration":4,"scene":{"type":"3d","objects":[
        {"id":"box","type":"box","position":[9,8,7],"keyframes":{
            "position.x":[[0,0],[1,10,"easeOutBack"],[2,20,"hold"],[4,40]],
            "position.y":[[0,5],[2,10,"easeInOut"]]
        }}
    ]}})).await;
    let clip = added["clips"][0]["id"].clone();
    ok(&s, Source::Window, "clip.update", json!({"clipId":clip,"speed":2})).await;
    ok(&s, Source::Window, "clip.trim", json!({"clipId":clip,"edge":"start","time":5.25})).await;
    let before = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let edited = ok(&s, Source::Window, "motion.updateKeyframes", json!({"clipId":clip,"updates":[
        {"id":"box","property":"position.x","time":5.5,"newTime":6,"value":15},
        {"id":"box","property":"position.x","time":6,"newTime":5.5},
        {"id":"box","property":"position.y","time":5,"newTime":7.5,"value":12,"easing":"easeOut"}
    ]})).await;
    assert_eq!(edited["updated"], 3);
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let kimchi_core::Scene::Space(scene) = kimchi_core::Scene::from_json(&after["scene"]).unwrap() else { panic!() };
    let object = &scene.objects[0];
    assert_eq!(object.position.0, [9.,8.,7.], "editing keys never changes unanimated base values");
    let x = &object.keyframes["position.x"];
    assert_eq!(x.iter().map(|k| (k.time,k.value.as_f64().unwrap())).collect::<Vec<_>>(), [(0.,0.),(1.,20.),(2.,15.),(4.,40.)]);
    assert_eq!(x[1].easing, kimchi_core::Easing::Hold);
    assert_eq!(x[2].easing, kimchi_core::Easing::parse("easeOutBack").unwrap());
    assert_eq!(object.keyframes["position.y"][1].time, 5., "keys beyond the clip end stay addressable");
    for invalid in [
        json!([{"id":"box","property":"position.x","time":5.5,"newTime":8},{"id":"missing","property":"x","time":0}]),
        json!([{"id":"box","property":"position.x","time":5.5},{"id":"box","property":"position.x","time":5.5}]),
        json!([{"id":"box","property":"position.x","time":5.5,"newTime":8},{"id":"box","property":"position.x","time":6,"newTime":8}]),
        json!([{"id":"box","property":"position.x","time":5.5,"value":"invalid number"}]),
        json!([{"id":"box","property":"position.x","time":5.5,"newtime":8}])
    ] {
        assert!(registry::call(&s, Source::Window, "motion.updateKeyframes", json!({"clipId":clip,"updates":invalid})).await.is_err());
        assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, after);
    }
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await, before);
    ok(&s, Source::Window, "motion.updateKeyframes", json!({"clipId":clip,"updates":[{"id":"box","property":"position.x","time":5.5,"newTime":7}]})).await;
    let after = ok(&s, Source::Cli, "motion.get", json!({"clipId":clip})).await;
    let kimchi_core::Scene::Space(scene) = kimchi_core::Scene::from_json(&after["scene"]).unwrap() else { panic!() };
    let x = &scene.objects[0].keyframes["position.x"];
    assert_eq!(x.len(), 3, "a moved key replaces an unselected destination");
    assert_eq!((x[2].time,x[2].value.as_f64()), (4.,Some(10.)));
}

#[tokio::test(flavor = "multi_thread")]
async fn every_command_has_a_handler() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    for spec in registry::commands() {
        if matches!(spec.name, "app.checkUpdates" | "app.installUpdate" | "generate.models" | "generate.check") {
            continue; // network
        }
        let cx = Ctx { source: Source::Window, spec };
        let r = crate::commands::dispatch(&s, &cx, Args::default()).await;
        if let Err(e) = r {
            assert!(!e.contains("not implemented"), "{}: {e}", spec.name);
        }
    }
}

#[test]
fn names_are_unique_and_well_formed() {
    let mut seen = std::collections::HashSet::new();
    for s in registry::commands() {
        assert!(seen.insert(s.name), "duplicate {}", s.name);
        let (family, verb) = s.name.split_once('.').expect("family.verb");
        assert!(family.chars().all(|c| c.is_ascii_lowercase()), "{}", s.name);
        assert!(verb.chars().next().unwrap().is_ascii_lowercase(), "{}", s.name);
        assert!(!s.doc.is_empty());
        for p in s.params {
            assert!(!p.doc.is_empty(), "{}.{}", s.name, p.name);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_share_one_undo_history() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Shared" })).await;
    ok(&s, Source::Agent, "clip.addText", json!({ "text": "Hello", "start": 0 })).await;
    ok(&s, Source::Cli, "clip.addSolid", json!({ "color": "#ff0000", "start": 4 })).await;
    let h = ok(&s, Source::Window, "history.list", json!({})).await;
    let sources: Vec<&str> = h["undo"].as_array().unwrap().iter().map(|s| s["source"].as_str().unwrap()).collect();
    assert_eq!(sources, ["cli", "agent"]);
    // The window undoes the CLI's edit, then the agent's.
    ok(&s, Source::Window, "history.undo", json!({})).await;
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(s.project().unwrap().clips().count(), 0);
}

/// What models send in place of JSON (Codex writes arrays of JSON text) is read as meant; arrays
/// say what their items are.
#[tokio::test(flavor = "multi_thread")]
async fn json_written_as_text_is_understood() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Mcp, "project.create", json!({})).await;
    let b = ok(&s, Source::Mcp, "project.batch", json!({ "commands": ["{\"command\": \"clip.addText\", \"params\": {\"text\": \"A\", \"start\": 0}}"] })).await;
    assert_eq!(b["results"].as_array().unwrap().len(), 1, "{b}");
    ok(&s, Source::Mcp, "clip.addText", json!({ "text": "B", "start": "4", "style": "{\"fontSize\": 40}" })).await;
    assert_eq!(ok(&s, Source::Mcp, "clip.get", json!({ "clipId": "B" })).await["start"], 4.0);
    ok(&s, Source::Mcp, "clip.delete", json!({ "clipIds": "B" })).await;
    let e = registry::call(&s, Source::Mcp, "project.renderFrame", json!({ "times": ["soon"] })).await.unwrap_err();
    assert!(e.contains("`times[0]` should be a number"), "{e}");
    let schema = registry::input_schema(registry::spec("clip.delete").unwrap());
    assert_eq!(schema["properties"]["clipIds"]["items"], json!({ "type": "string" }));
}

#[tokio::test(flavor = "multi_thread")]
async fn names_work_like_ids_and_mistakes_are_explained() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Cli, "clip.addText", json!({ "text": "Opening title", "start": 1 })).await;
    let c = ok(&s, Source::Cli, "clip.update", json!({ "clipId": "Opening title", "opacity": 0.5, "style": { "fontSize": 80 } })).await;
    assert_eq!(c["transform"]["opacity"], 0.5);
    let e = registry::call(&s, Source::Cli, "clip.update", json!({ "clipId": "Opening titel" })).await.unwrap_err();
    assert!(e.contains("Did you mean Opening title"), "{e}");
    let e = registry::call(&s, Source::Cli, "clip.updat", json!({})).await.unwrap_err();
    assert!(e.contains("clip.update"), "{e}");
    let e = registry::call(&s, Source::Cli, "clip.split", json!({ "tme": 2 })).await.unwrap_err();
    assert!(e.contains("Did you mean `time`"), "{e}");
    let e = registry::call(&s, Source::Cli, "clip.split", json!({ "time": "two" })).await.unwrap_err();
    assert!(e.contains("should be a number"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_is_one_step_and_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Agent, "project.batch", json!({ "commands": [
        { "command": "clip.addSolid", "params": { "color": "#000000", "start": 0 } },
        { "command": "timeline.addMarker", "params": { "time": 1, "label": "Drop" } },
    ]})).await;
    assert_eq!(s.read(|ed| ed.undo_steps().len()).unwrap(), 1);
    let e = registry::call(&s, Source::Agent, "project.batch", json!({ "commands": [
        { "command": "timeline.addMarker", "params": { "time": 2, "label": "B" } },
        { "command": "clip.delete", "params": { "clipIds": ["nope"] } },
    ]})).await.unwrap_err();
    assert!(e.contains("Nothing was changed"), "{e}");
    assert_eq!(s.project().unwrap().markers.len(), 1);
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert!(s.project().unwrap().markers.is_empty());
    assert_eq!(s.project().unwrap().clips().count(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn permissions_apply_to_agents_and_mcp_only() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    let e = registry::call(&s, Source::Mcp, "generate.setKey", json!({ "provider": "fal", "key": "x" })).await.unwrap_err();
    assert!(e.contains("stays with the person"), "{e}");
    let e = registry::call(&s, Source::Agent, "app.setSetting", json!({ "key": "updates.checkOnStart", "value": false })).await.unwrap_err();
    assert!(e.contains("\"settings\" permission"), "{e}");
    s.update_settings(|st| st.agent.permissions.enabled = false).unwrap();
    let e = registry::call(&s, Source::Mcp, "project.overview", json!({})).await.unwrap_err();
    assert!(e.contains("turned off"), "{e}");
    // The person's own tools are not agents.
    ok(&s, Source::Cli, "app.setSetting", json!({ "key": "updates.checkOnStart", "value": false })).await;
    assert!(!s.settings().updates.check_on_start);
}

#[tokio::test(flavor = "multi_thread")]
async fn file_mode_saves_back_to_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({ "name": "On disk" })).await;
    let path = dir.path().join("cut.json");
    ok(&s, Source::Cli, "project.saveAs", json!({ "path": path })).await;
    let s2 = session(&dir.path().join("other"));
    ok(&s2, Source::Cli, "project.open", json!({ "path": path })).await;
    ok(&s2, Source::Cli, "timeline.addMarker", json!({ "time": 3, "label": "Here" })).await;
    let back = crate::session::read_project_file(&path).unwrap();
    assert_eq!(back.markers[0].label, "Here");
}

#[tokio::test(flavor = "multi_thread")]
async fn overview_reports_problems() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Cli, "clip.addText", json!({ "text": "Hi", "start": 0 })).await;
    let track = ok(&s, Source::Cli, "track.list", json!({})).await[0]["name"].as_str().unwrap().to_string();
    ok(&s, Source::Cli, "track.update", json!({ "trackId": track, "hidden": true })).await;
    let o = ok(&s, Source::Mcp, "project.overview", json!({})).await;
    assert!(o["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("hidden")), "{o}");
    assert_eq!(o["tracks"][0]["clips"][0]["text"], "Hi");
}

#[tokio::test(flavor = "multi_thread")]
async fn window_commands_need_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let e = registry::call(&s, Source::Cli, "timeline.play", json!({})).await.unwrap_err();
    assert!(e.contains("needs the kimchi window"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bridge_runs_commands_for_a_client_with_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let path = dir.path().join("control.json");
    let _server = crate::bridge::Server::start_at(s.clone(), path.clone()).await.unwrap();
    let mut c = crate::bridge::Client::connect(&path, "mcp").await.unwrap();
    c.call("project.create", json!({ "name": "Live" })).await.unwrap();
    let list = c.call("project.list", json!({})).await.unwrap();
    assert_eq!(list[0]["name"], "Live");
    // A wrong token is refused.
    let mut d = crate::bridge::read_discovery(&path).unwrap();
    d.token = "0".repeat(d.token.len());
    std::fs::write(&path, serde_json::to_vec(&d).unwrap()).unwrap();
    assert!(crate::bridge::Client::connect(&path, "cli").await.is_err());
}

#[test]
fn markdown_lists_every_command() {
    let md = registry::markdown();
    for s in registry::commands() {
        assert!(md.contains(&format!("### `{}`", s.name)));
    }
}

/// kimchi → ryolune → kimchi through a fake ryolune bridge that records what it is asked.
#[tokio::test(flavor = "multi_thread")]
async fn hands_the_cut_to_ryolune_and_takes_audio_back() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    if kimchi_media::Tools::locate().is_err() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // A fake ryolune: answers every call and remembers it.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let control = dir.path().join("ryolune-control.json");
    std::fs::write(&control, serde_json::to_vec(&json!({ "version": 1, "port": port, "token": "t", "pid": 1 })).unwrap()).unwrap();
    // SAFETY: only this test reads these variables.
    unsafe {
        std::env::set_var("RYOLUNE_CONTROL", &control);
        std::env::set_var("LSUITE_HOME", dir.path().join("lsuite"));
    }
    let calls = Arc::new(parking_lot::Mutex::new(Vec::<Value>::new()));
    let seen = calls.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let (r, mut w) = stream.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Value = serde_json::from_str(&line).unwrap();
                    let method = req["method"].as_str().unwrap().to_string();
                    let result = match method.as_str() {
                        "session.info" => json!({ "transport": { "tempo": 120.0, "timeSignature": { "numerator": 4, "denominator": 4 } } }),
                        "session.bounce" => {
                            let out = req["params"]["path"].as_str().unwrap().to_string();
                            let tools = kimchi_media::Tools::locate().unwrap();
                            std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=330:duration=2", &out]).status().unwrap();
                            json!({ "path": out })
                        }
                        _ => json!({ "ok": true }),
                    };
                    let answer = json!({ "jsonrpc": "2.0", "id": req["id"], "result": result });
                    seen.lock().push(req);
                    w.write_all(format!("{answer}\n").as_bytes()).await.unwrap();
                }
            });
        }
    });

    ok(&s, Source::Cli, "project.create", json!({ "name": "Score me" })).await;
    let wav = dir.path().join("tone.wav");
    let tools = kimchi_media::Tools::locate().unwrap();
    std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=4"]).arg(&wav).status().unwrap();
    ok(&s, Source::Cli, "media.import", json!({ "paths": [wav], "place": true, "start": 0 })).await;
    ok(&s, Source::Cli, "timeline.addMarker", json!({ "time": 2, "label": "Drop" })).await;

    let r = ok(&s, Source::Cli, "handoff.toRyolune", json!({})).await;
    assert_eq!(r["sentToRyolune"], true, "{r}");
    assert!(std::path::Path::new(r["audio"].as_str().unwrap()).is_file());
    let cut: Value = serde_json::from_slice(&std::fs::read(r["cut"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(cut["markers"][0]["label"], "Drop");
    {
        let calls = calls.lock();
        let methods: Vec<&str> = calls.iter().map(|c| c["method"].as_str().unwrap()).collect();
        assert_eq!(methods, ["auth", "session.info", "session.importAudio", "marker.add"]);
        // 2 s at 120 bpm in 4/4 is bar 1 (zero-based).
        assert!((calls[3]["params"]["bar"].as_f64().unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(calls[3]["params"]["name"], "Drop");
    }

    let back = ok(&s, Source::Cli, "handoff.fromRyolune", json!({ "start": 1 })).await;
    let clip = back["clips"][0].as_str().unwrap().parse().unwrap();
    let p = s.project().unwrap();
    assert_eq!(p.clip(clip).unwrap().start, 1.0);
    assert_eq!(p.tracks[p.locate_clip(clip).unwrap().0].kind, kimchi_core::TrackKind::Audio);
}

/// A terminal agent (MCP over the bridge): its records name the clips it created and carry one
/// checkpoint, taken before its first change, that reverts the whole session.
#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_session_can_be_reverted_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Mine" })).await;
    ok(&s, Source::Window, "clip.addSolid", json!({ "color": "#111111", "start": 0 })).await;
    let path = dir.path().join("control.json");
    let _server = crate::bridge::Server::start_at(s.clone(), path.clone()).await.unwrap();
    let mut rx = s.subscribe();
    let mut c = crate::bridge::Client::connect(&path, "mcp").await.unwrap();
    c.call("project.overview", json!({})).await.unwrap();
    c.call("clip.addText", json!({ "text": "One", "start": 1 })).await.unwrap();
    c.call("clip.addText", json!({ "text": "Two", "start": 6 })).await.unwrap();
    let mut records = vec![];
    while let Ok(Ok(e)) = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
        if let crate::Event::Command { record } = e {
            records.push(record);
        }
    }
    let adds: Vec<_> = records.iter().filter(|r| r.command == "clip.addText").collect();
    assert_eq!(adds.len(), 2);
    assert_eq!(adds[0].created.len(), 1, "{:?}", adds[0]);
    let cp = adds[0].checkpoint.expect("a checkpoint for the session");
    assert_eq!(adds[1].checkpoint, Some(cp), "one checkpoint per connection");
    ok(&s, Source::Window, "history.revertTo", json!({ "checkpoint": cp })).await;
    let p = s.project().unwrap();
    assert_eq!(p.clips().count(), 1, "only the person's solid is left");
}

#[tokio::test(flavor = "multi_thread")]
async fn motion_clips_templates_and_keyframes() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Motion", "width": 640, "height": 360 })).await;

    // A template clip, its scene, a layer changed by hand, then new template values.
    let added = ok(&s, Source::Agent, "motion.addTemplate", json!({ "template": "lowerThird", "values": { "title": "Grace Hopper" }, "start": 0 })).await;
    let clip = added["clips"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(added["clips"][0]["type"], "motion");
    assert_eq!(added["clips"][0]["template"], "lowerThird");
    let got = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "title" })).await;
    assert_eq!(got["text"], "Grace Hopper");
    ok(&s, Source::Agent, "motion.setLayer", json!({ "clipId": clip, "layer": { "id": "dot", "type": "ellipse", "width": 20, "height": 20, "fill": "#ffffff" }, "parent": "lowerThird" })).await;
    ok(&s, Source::Agent, "motion.setKeyframes", json!({ "clipId": clip, "id": "dot", "property": "opacity", "keyframes": [[0, 0], [0.5, 1, "easeOut"]] })).await;
    let scene = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip })).await;
    assert!(scene["scene"].to_string().contains("\"dot\""));
    ok(&s, Source::Agent, "motion.setTemplate", json!({ "clipId": clip, "values": { "subtitle": "Rear admiral" } })).await;
    let got = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "subtitle" })).await;
    assert_eq!(got["text"], "Rear admiral");
    assert_eq!(ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "title" })).await["text"], "Grace Hopper", "values are kept");

    // The template was re-made (the dot went with the old scene) and the clip's name followed it.
    assert!(ok(&s, Source::Agent, "clip.get", json!({ "clipId": clip })).await["name"].as_str().unwrap().ends_with("Grace Hopper"));
    ok(&s, Source::Agent, "motion.setLayer", json!({ "clipId": clip, "layer": { "id": "dot", "type": "ellipse", "width": 20, "height": 20, "keyframes": { "opacity": [[0, 0], [0.5, 1]] } } })).await;
    // One property at a time; animated ones get a keyframe at that time.
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": clip, "id": "dot", "props": { "x": 40, "fill": "#ff0000", "stroke": { "width": 3 } } })).await;
    let dot = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "dot" })).await;
    assert_eq!((dot["x"].as_f64(), dot["fill"].as_str(), dot["stroke"]["width"].as_f64()), (Some(40.0), Some("#ff0000"), Some(3.0)));
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": clip, "id": "dot", "props": { "opacity": 0.5 }, "time": 2.0 })).await;
    let dot = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "dot" })).await;
    assert_eq!(dot["keyframes"]["opacity"].as_array().unwrap().len(), 3, "a third keyframe at 2 s: {dot}");
    ok(&s, Source::Window, "motion.addKeyframe", json!({ "clipId": clip, "id": "dot", "property": "x", "time": 1.0 })).await;
    ok(&s, Source::Window, "motion.addKeyframe", json!({ "clipId": clip, "id": "dot", "property": "x", "time": 2.0, "value": 90, "easing": "easeOut" })).await;
    ok(&s, Source::Window, "motion.removeKeyframe", json!({ "clipId": clip, "id": "dot", "property": "x", "time": 1.0 })).await;
    let dot = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "dot" })).await;
    assert_eq!(dot["keyframes"]["x"].as_array().unwrap().len(), 1);
    let e = registry::call(&s, Source::Window, "motion.addKeyframe", json!({ "clipId": clip, "id": "dot", "property": "wobble" })).await.unwrap_err();
    assert!(e.contains("no property `wobble`"), "{e}");

    // Mistakes come back with the fix.
    let e = registry::call(&s, Source::Agent, "motion.add", json!({ "scene": { "layers": [{ "id": "a", "type": "rectangle" }] } })).await.unwrap_err();
    assert!(e.contains("Did you mean `rect`"), "{e}");
    let e = registry::call(&s, Source::Agent, "motion.addTemplate", json!({ "template": "lowerthird3" })).await.unwrap_err();
    assert!(e.contains("lowerThird"), "{e}");

    // A 3D scene as a clip; its length follows the keyframes.
    let three = ok(&s, Source::Agent, "motion.add", json!({ "start": 6, "scene": {
        "objects": [{ "id": "cube", "type": "box", "keyframes": { "rotation.y": [[0, 0], [4, 360]] } }]
    } })).await;
    assert_eq!(three["clips"][0]["scene"], "3d");
    assert_eq!(three["clips"][0]["duration"], 5.0);
    let three_id = three["clips"][0]["id"].as_str().unwrap().to_string();
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": three_id, "id": "camera", "props": { "fov": 30, "position": [0, 2, 9] } })).await;
    ok(&s, Source::Window, "motion.updateLayer", json!({ "clipId": three_id, "id": "cube", "props": { "material": { "metallic": 1 }, "color": "#00ff00" } })).await;
    let cube = ok(&s, Source::Agent, "motion.get", json!({ "clipId": three_id, "id": "cube" })).await;
    assert_eq!((cube["material"]["metallic"].as_f64(), cube["material"]["color"].as_str()), (Some(1.0), Some("#00ff00")));
    let cam = ok(&s, Source::Agent, "motion.get", json!({ "clipId": three_id, "id": "camera" })).await;
    assert_eq!((cam["fov"].as_f64(), cam["position"][1].as_f64()), (Some(30.0), Some(2.0)));

    // Clip keyframes and presets, one undo step each.
    let text = ok(&s, Source::Agent, "clip.addText", json!({ "text": "Hi", "start": 0, "duration": 3 })).await;
    let text_id = text["clips"][0]["id"].as_str().unwrap().to_string();
    let r = ok(&s, Source::Agent, "clip.setKeyframes", json!({ "clipId": text_id, "property": "x", "keyframes": [{ "time": 0, "value": -200 }, { "time": 1, "value": 0, "easing": "easeOutBack" }] })).await;
    assert_eq!(r["keyframes"]["x"][1]["easing"], "easeOutBack");
    let e = registry::call(&s, Source::Agent, "clip.setKeyframes", json!({ "clipId": text_id, "property": "wobble", "keyframes": [[0, 1]] })).await.unwrap_err();
    assert!(e.contains("can't animate `wobble`"), "{e}");
    ok(&s, Source::Agent, "clip.animate", json!({ "clipIds": [text_id], "preset": "fadeOut" })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": text_id })).await;
    assert!(c["keyframes"]["opacity"].as_array().unwrap().len() == 2 && c["keyframes"]["x"].is_array());
    ok(&s, Source::Agent, "history.undo", json!({})).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": text_id })).await;
    assert!(c["keyframes"]["opacity"].is_null(), "the preset was one step");
    ok(&s, Source::Agent, "clip.addKeyframe", json!({ "clipId": text_id, "property": "opacity", "time": 2 })).await;
    ok(&s, Source::Agent, "clip.removeKeyframe", json!({ "clipId": text_id, "property": "x" })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": text_id })).await;
    assert!(c["keyframes"]["x"].is_null() && c["keyframes"]["opacity"].is_array());

    // The guide and listings.
    let g = ok(&s, Source::Agent, "motion.guide", json!({ "topic": "3d" })).await;
    assert!(g["guide"].as_str().unwrap().contains("## 3D scenes") && !g["guide"].as_str().unwrap().contains("## 2D scenes"));
    assert!(ok(&s, Source::Agent, "motion.templates", json!({})).await.as_array().unwrap().len() >= 15);
    assert!(ok(&s, Source::Agent, "motion.presets", json!({})).await.as_array().unwrap().len() >= 30);

    // Frames to look at (needs ffmpeg only for media; these are all drawn).
    if s.tools().is_ok() {
        let one = ok(&s, Source::Agent, "project.renderFrame", json!({ "time": 1.0, "width": 320 })).await;
        assert!(std::path::Path::new(one["path"].as_str().unwrap()).is_file());
        let sheet = ok(&s, Source::Agent, "project.renderFrame", json!({ "times": [0.2, 1.0, 2.0, 7.0] })).await;
        let png = kimchi_media::tiny_skia::Pixmap::load_png(sheet["path"].as_str().unwrap()).unwrap();
        assert!(png.width() > 900 && png.height() > 500, "2×2 sheet: {}x{}", png.width(), png.height());
        let frame = ok(&s, Source::Agent, "media.frame", json!({ "clipId": clip, "time": 2.0 })).await;
        assert!(std::path::Path::new(frame["path"].as_str().unwrap()).is_file());
    }
}

/// export.encoders says what each format is encoded with here; export.start takes the choice and
/// its status names the encoder that ran.
#[tokio::test(flavor = "multi_thread")]
async fn exports_name_their_encoder() {
    if kimchi_media::Tools::locate().is_err() {
        eprintln!("ffmpeg not found; skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    let enc = ok(&s, Source::Cli, "export.encoders", json!({})).await;
    let mp4 = enc["formats"].as_array().unwrap().iter().find(|f| f["format"] == "mp4").unwrap().clone();
    assert_eq!(mp4["software"]["id"], "libx264", "{enc}");
    assert_eq!(mp4["software"]["label"], "x264 (CPU)");
    assert_eq!(mp4["auto"], if mp4["hardware"].is_null() { mp4["software"].clone() } else { mp4["hardware"].clone() });

    ok(&s, Source::Cli, "project.create", json!({ "name": "Encoders" })).await;
    ok(&s, Source::Cli, "clip.addSolid", json!({ "color": "#336699", "start": 0, "duration": 1 })).await;
    let out = dir.path().join("cpu.mp4");
    let st = ok(&s, Source::Cli, "export.start", json!({ "path": out, "encoder": "software", "width": 320, "height": 180 })).await;
    assert_eq!((st["done"].as_bool(), st["encoder"].as_str()), (Some(true), Some("libx264")), "{st}");
    assert!(out.is_file());
    let err = registry::call(&s, Source::Cli, "export.start", json!({ "path": out, "encoder": "quantum" })).await.unwrap_err();
    assert!(err.contains("auto, hardware or software"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn looks_from_other_apps() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Grade", "width": 640, "height": 360 })).await;
    let a = ok(&s, Source::Window, "clip.addSolid", json!({ "color": "#808080", "start": 0, "duration": 3 })).await["clips"][0]["id"].as_str().unwrap().to_string();
    // A folder with a LUT in a subfolder, a Lightroom preset and a file that isn't a look.
    let looks = dir.path().join("My Looks");
    std::fs::create_dir_all(looks.join("Film")).unwrap();
    std::fs::write(looks.join("Film/Amber.3dl"), "0 1023\n0 0 0\n0 0 4095\n0 4095 0\n0 4095 4095\n4095 0 0\n4095 0 4095\n4095 4095 0\n4095 4095 4095\n").unwrap();
    std::fs::write(
        looks.join("Pop.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Contrast2012="+30" crs:Saturation="+20" crs:Clarity2012="+10"/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    std::fs::write(looks.join("readme.txt"), "hi").unwrap();
    let r = ok(&s, Source::Window, "looks.import", json!({ "paths": [looks] })).await;
    let added = r["added"].as_array().unwrap();
    assert_eq!(added.len(), 2, "{r}");
    assert!(added.iter().any(|l| l["folder"] == "My Looks/Film" && l["name"] == "Amber"));
    assert!(added.iter().any(|l| l["name"] == "Pop" && l["dropped"][0].as_str().unwrap().contains("Clarity")));
    let list = ok(&s, Source::Agent, "looks.list", json!({})).await;
    assert_eq!(list.as_array().unwrap().len(), kimchi_core::effects::LOOKS.len() + 2);
    assert_eq!(ok(&s, Source::Agent, "looks.list", json!({ "query": "lightroom" })).await.as_array().unwrap().len(), 1);
    // Applying: corrections and LUT replaced, one step; clip.setEffects takes library looks too.
    ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "chromaKey": true })).await;
    ok(&s, Source::Agent, "looks.apply", json!({ "clipIds": [a], "look": "Amber", "strength": 0.5 })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": a })).await;
    assert_eq!(c["effects"]["lut"]["strength"].as_f64(), Some(0.5));
    assert!(c["effects"]["chroma_key"].is_object(), "the key stays");
    let c = ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "look": "pop" })).await;
    assert_eq!(c["clips"][0]["effects"]["contrast"].as_f64(), Some(0.3));
    let e = registry::call(&s, Source::Agent, "looks.apply", json!({ "clipIds": [a], "look": "Wram" })).await.unwrap_err();
    assert!(e.to_lowercase().contains("did you mean “warm”"), "{e}");
    // Saving: into the library, and as a .cube.
    let saved = ok(&s, Source::Window, "looks.save", json!({ "clipId": a, "name": "Mine" })).await;
    assert_eq!(saved["folder"], "Saved");
    let cube = dir.path().join("out/mine.cube");
    let r = ok(&s, Source::Window, "looks.save", json!({ "clipId": a, "path": cube })).await;
    assert!(r["notIncluded"].as_array().unwrap().iter().any(|x| x == "the chroma key"));
    assert!(kimchi_media::render::grade::lut(&cube).is_ok());
    ok(&s, Source::Window, "looks.remove", json!({ "look": "Mine" })).await;
    assert!(registry::call(&s, Source::Window, "looks.remove", json!({ "look": "noir" })).await.unwrap_err().contains("built-in"));
    // Export presets: listed with the size they give this project; unknown ones explained.
    let presets = ok(&s, Source::Agent, "export.presets", json!({})).await;
    let yt = presets.as_array().unwrap().iter().find(|p| p["id"] == "youtube-1080p").unwrap();
    assert_eq!((yt["params"]["width"].as_u64(), yt["params"]["height"].as_u64(), yt["params"]["loudness"].as_f64()), (Some(1920), Some(1080), Some(-14.0)));
    let e = registry::call(&s, Source::Window, "export.start", json!({ "path": dir.path().join("x.mp4"), "preset": "youtub" })).await.unwrap_err();
    assert!(e.contains("youtube"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn effects_transitions_and_freeze_frames() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Cut", "width": 640, "height": 360 })).await;
    let a = ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#ff0000", "start": 0, "duration": 3, "trackId": "Video 1" })).await["clips"][0]["id"].as_str().unwrap().to_string();
    let b = ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#0000ff", "start": 3, "duration": 3, "trackId": "Video 1" })).await["clips"][0]["id"].as_str().unwrap().to_string();

    // Effects: a look, a field on top, a key, a LUT; mistakes explained.
    let r = ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "look": "vintage", "contrast": 0.4, "chromaKey": "#00ff00" })).await;
    let fx = &r["clips"][0]["effects"];
    assert_eq!((fx["contrast"].as_f64(), fx["vignette"].as_f64(), fx["chroma_key"]["color"].as_str()), (Some(0.4), Some(0.45), Some("#00ff00")));
    let e = registry::call(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "look": "noire" })).await.unwrap_err();
    assert!(e.contains("noir"), "{e}");
    let cube = dir.path().join("id.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for i in 0..8 {
        text.push_str(&format!("{} {} {}\n", i & 1, (i >> 1) & 1, (i >> 2) & 1));
    }
    std::fs::write(&cube, text).unwrap();
    ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "lut": cube, "lutStrength": 0.5 })).await;
    let e = registry::call(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "lut": dir.path().join("nope.cube") })).await.unwrap_err();
    assert!(e.contains("Can't use"), "{e}");
    // Effects animate like other properties, and removing the animation keeps the value.
    ok(&s, Source::Agent, "clip.setKeyframes", json!({ "clipId": a, "property": "saturation", "keyframes": [[0, -1], [2, 1]] })).await;
    ok(&s, Source::Agent, "clip.removeKeyframe", json!({ "clipId": a, "property": "saturation" })).await;
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": a })).await;
    assert!(c["keyframes"]["saturation"].is_null());
    assert_eq!(c["effects"]["saturation"].as_f64(), Some(-1.0), "the value at the playhead (0 s) stays");
    let reset = ok(&s, Source::Agent, "clip.setEffects", json!({ "clipIds": [a], "reset": true })).await;
    assert_eq!(reset["clips"][0]["effects"], json!(null), "no effects left: {reset}");
    assert_eq!(ok(&s, Source::Agent, "clip.looks", json!({})).await.as_array().unwrap().len(), kimchi_core::effects::LOOKS.len());

    // Transitions: on every cut of a track, listed with where they play, then changed and removed.
    let r = ok(&s, Source::Agent, "transition.set", json!({ "trackId": "Video 1", "kind": "crossfade", "duration": 1 })).await;
    assert_eq!(r.as_array().unwrap().len(), 1);
    assert_eq!((r[0]["kind"].as_str(), r[0]["start"].as_f64(), r[0]["end"].as_f64(), r[0]["from"].as_str()), (Some("dissolve"), Some(2.5), Some(3.5), Some("Solid")));
    ok(&s, Source::Agent, "transition.set", json!({ "clipIds": [b], "kind": "pushLeft" })).await;
    let l = ok(&s, Source::Agent, "transition.list", json!({})).await;
    assert_eq!((l[0]["kind"].as_str(), l[0]["duration"].as_f64()), (Some("pushLeft"), Some(1.0)), "the length stayed");
    let e = registry::call(&s, Source::Agent, "transition.set", json!({ "clipIds": [b], "kind": "swirl" })).await.unwrap_err();
    assert!(e.contains("Transitions:"), "{e}");
    if s.tools().is_ok() {
        // Halfway through a push the red clip is on the left, the blue one on the right.
        let f = ok(&s, Source::Agent, "project.renderFrame", json!({ "time": 3.0, "width": 320 })).await;
        let png = kimchi_media::tiny_skia::Pixmap::load_png(f["path"].as_str().unwrap()).unwrap();
        let (l, r) = (png.pixel(40, 90).unwrap(), png.pixel(280, 90).unwrap());
        assert!(l.red() > 200 && l.blue() < 50 && r.blue() > 200 && r.red() < 50, "{l:?} {r:?}");
    }
    ok(&s, Source::Agent, "transition.remove", json!({ "clipIds": [b] })).await;
    assert!(ok(&s, Source::Agent, "transition.list", json!({})).await.as_array().unwrap().is_empty());

    // Solids and titles can't play backwards.
    let e = registry::call(&s, Source::Agent, "clip.update", json!({ "clipId": a, "reverse": true })).await.unwrap_err();
    assert!(e.contains("backwards"), "{e}");

    // A freeze frame splits the clip, holds for 2 s and pushes the rest later, as one step.
    if s.tools().is_ok() {
        let r = ok(&s, Source::Agent, "clip.freezeFrame", json!({ "clipId": a, "time": 1.0 })).await;
        assert_eq!((r["clips"][0]["start"].as_f64(), r["clips"][0]["duration"].as_f64()), (Some(1.0), Some(2.0)));
        let clips = ok(&s, Source::Agent, "clip.list", json!({ "trackId": "Video 1" })).await;
        let starts: Vec<f64> = clips.as_array().unwrap().iter().map(|c| c["start"].as_f64().unwrap()).collect();
        assert_eq!(starts, vec![0.0, 1.0, 3.0, 5.0]);
        // The still in the media list says what it is.
        let media = ok(&s, Source::Agent, "media.list", json!({})).await;
        assert!(media.as_array().unwrap().iter().any(|m| m["name"].as_str().is_some_and(|n| n.ends_with("· frame at 1.00 s"))), "{media}");
        ok(&s, Source::Agent, "history.undo", json!({})).await;
        assert_eq!(ok(&s, Source::Agent, "clip.list", json!({ "trackId": "Video 1" })).await.as_array().unwrap().len(), 2);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn captions_import_style_and_export() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Subs", "width": 640, "height": 360 })).await;
    let srt = dir.path().join("in.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,500\nHello\n\n2\n00:00:03,000 --> 00:00:04,000\nSecond line\nof two\n").unwrap();
    let r = ok(&s, Source::Agent, "captions.import", json!({ "path": srt })).await;
    assert_eq!(r["captions"], 2);
    let tracks = ok(&s, Source::Agent, "track.list", json!({})).await;
    assert_eq!(tracks[0]["name"], "Captions", "a captions track on top: {tracks}");
    let list = ok(&s, Source::Agent, "captions.list", json!({})).await;
    assert_eq!((list[1]["text"].as_str(), list[1]["start"].as_f64()), (Some("Second line\nof two"), Some(3.0)));
    // One more, styled like the others; then all restyled at once.
    ok(&s, Source::Agent, "captions.add", json!({ "text": "Third", "start": 5 })).await;
    ok(&s, Source::Agent, "captions.setStyle", json!({ "style": { "fontSize": 30, "color": "#ffcc00" }, "y": 100 })).await;
    let p = s.project().unwrap();
    assert!(p.captions().iter().all(|(_, c, _)| c.transform.y == 100.0 && matches!(&c.content, kimchi_core::ClipContent::Text { style } if style.font_size == 30.0 && style.color == "#ffcc00")));
    // Out as WebVTT, and next to an export instead of in the picture.
    let vtt = dir.path().join("out.vtt");
    ok(&s, Source::Agent, "captions.export", json!({ "path": vtt })).await;
    let text = std::fs::read_to_string(&vtt).unwrap();
    assert!(text.starts_with("WEBVTT") && text.contains("00:00:05.000 --> 00:00:07.500\nThird"), "{text}");
    if s.tools().is_ok() {
        let mp4 = dir.path().join("cut.mp4");
        ok(&s, Source::Agent, "export.start", json!({ "path": mp4, "quality": "draft", "captions": "file", "wait": true })).await;
        assert!(dir.path().join("cut.srt").is_file());
    }
    // Clearing is one step.
    ok(&s, Source::Agent, "captions.clear", json!({})).await;
    assert!(ok(&s, Source::Agent, "captions.list", json!({})).await.as_array().unwrap().is_empty());
    ok(&s, Source::Agent, "history.undo", json!({})).await;
    assert_eq!(ok(&s, Source::Agent, "captions.list", json!({})).await.as_array().unwrap().len(), 3);
    let models = ok(&s, Source::Agent, "captions.models", json!({})).await;
    assert_eq!(models.as_array().unwrap().len(), 3);
    assert_eq!(ok(&s, Source::Agent, "captions.status", json!({})).await["running"], false);
}

/// The real model on real speech: `KIMCHI_SPEECH_WAV=jfk.wav KIMCHI_WHISPER_DIR=/models cargo test
/// -p kimchi-control captions_from_speech -- --ignored`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn captions_from_speech() {
    let (Ok(wav), Ok(models)) = (std::env::var("KIMCHI_SPEECH_WAV"), std::env::var("KIMCHI_WHISPER_DIR")) else { return };
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // Reuse the downloaded models.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&models, dir.path().join("data/models")).unwrap();
    ok(&s, Source::Window, "project.create", json!({ "name": "Speech" })).await;
    ok(&s, Source::Agent, "media.import", json!({ "paths": [wav], "place": true, "start": 2 })).await;
    let r = ok(&s, Source::Agent, "captions.transcribe", json!({})).await;
    eprintln!("{r:#}");
    assert_eq!(r["language"], "en");
    let list = ok(&s, Source::Agent, "captions.list", json!({})).await;
    let text = list.as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap().replace('\n', " ")).collect::<Vec<_>>().join(" ").to_lowercase();
    assert!(text.contains("ask not what your country can do for you"), "{text}");
    assert!(list[0]["start"].as_f64().unwrap() >= 2.0, "timed on the timeline: {list}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_move_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({})).await;
    let a = registry::created_clips(&ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#111111", "start": 0 })).await)[0];
    let b = registry::created_clips(&ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#222222", "start": 10 })).await)[0];
    let tracks = ok(&s, Source::Agent, "track.list", json!({})).await;
    let audio = tracks.as_array().unwrap().iter().find(|t| t["kind"] == "audio").unwrap()["id"].as_str().unwrap().to_string();
    let moves = json!({ "moves": [{ "clipId": a.to_string(), "start": 30 }, { "clipId": b.to_string(), "trackId": audio, "start": 0 }] });
    assert!(registry::call(&s, Source::Agent, "clip.moveMany", moves).await.is_err());
    let p = s.project().unwrap();
    assert_eq!(p.clip(a).unwrap().start, 0.0, "the first move is put back");
    assert_eq!(s.read(|ed| ed.undo_steps().len()).unwrap(), 2);
    // Absurd times are refused rather than written as null.
    assert!(registry::call(&s, Source::Agent, "timeline.addMarker", json!({ "time": 1e300 })).await.is_err());
    let e = registry::call(&s, Source::Agent, "generate.wait", json!({ "jobId": "nope", "timeout": 1e20 })).await.unwrap_err();
    assert!(e.contains("No job"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn batches_hold_others_back_and_end_when_dropped() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({})).await;
    // `timeline.play` waits for the window, which answers when the test says.
    let mut ui = s.attach_ui();
    let batch = |label: &'static str| {
        let s = s.clone();
        async move {
            registry::call(&s, Source::Agent, "project.batch", json!({ "commands": [
                { "command": "timeline.addMarker", "params": { "time": 1, "label": label } },
                { "command": "timeline.play" },
            ]}))
            .await
        }
    };
    let markers = |s: &Arc<Session>| s.project().unwrap().markers.iter().map(|m| m.label.clone()).collect::<Vec<_>>();
    let running = tokio::spawn(batch("agent"));
    let call = ui.next().await.unwrap();
    assert_eq!(markers(&s), ["agent"]);
    // The window's change waits for the batch instead of folding into it.
    let s2 = s.clone();
    let window = tokio::spawn(async move { registry::call(&s2, Source::Window, "timeline.addMarker", json!({ "time": 2, "label": "window" })).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!window.is_finished());
    call.reply.send(Err("no".into())).unwrap();
    assert!(running.await.unwrap().is_err());
    window.await.unwrap().unwrap();
    assert_eq!(markers(&s), ["window"], "the batch rolled back only its own change");
    // A batch whose caller stops waiting (an agent's Stop) is rolled back, not left open.
    assert!(tokio::time::timeout(std::time::Duration::from_millis(300), batch("dropped")).await.is_err());
    assert!(!s.read(|ed| ed.in_batch()).unwrap());
    assert_eq!(markers(&s), ["window"]);
    ok(&s, Source::Window, "timeline.addMarker", json!({ "time": 3, "label": "after" })).await;
    assert_eq!(s.read(|ed| ed.undo_steps().len()).unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_keep_to_themselves() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // A checkpoint names a state of its own project only.
    ok(&s, Source::Window, "project.create", json!({ "name": "A" })).await;
    let cp = ok(&s, Source::Window, "history.checkpoint", json!({})).await["checkpoint"].clone();
    ok(&s, Source::Window, "project.create", json!({ "name": "B" })).await;
    let e = registry::call(&s, Source::Window, "history.revertTo", json!({ "checkpoint": cp })).await.unwrap_err();
    assert!(e.contains("isn't in the open project's history"), "{e}");
    // Renaming a project opened from a file answers with its new name.
    let file = dir.path().join("b.json");
    ok(&s, Source::Window, "project.saveAs", json!({ "path": file })).await;
    ok(&s, Source::Window, "project.open", json!({ "path": file })).await;
    assert_eq!(ok(&s, Source::Window, "project.rename", json!({ "name": "Renamed" })).await["name"], "Renamed");
    // A duplicate has its own copy of the generated media.
    ok(&s, Source::Window, "project.create", json!({ "name": "Orig" })).await;
    let orig = s.current_id().unwrap();
    let gen_dir = s.library.generated_dir(orig);
    std::fs::create_dir_all(&gen_dir).unwrap();
    std::fs::write(gen_dir.join("pic.png"), b"png").unwrap();
    let asset = kimchi_core::Asset {
        id: kimchi_core::new_id(),
        name: "pic".into(),
        kind: kimchi_core::MediaKind::Image,
        path: gen_dir.join("pic.png").to_string_lossy().into_owned(),
        meta: Default::default(),
        origin: kimchi_core::AssetOrigin::Imported,
        created_at: chrono::Utc::now(),
        thumbnail: None,
        filmstrip: None,
        waveform: None,
        proxy: None,
        beats: None,
    };
    s.apply("test", Source::Window, &kimchi_core::Edit::AddAsset { asset }, None).unwrap();
    let copy = ok(&s, Source::Window, "project.duplicate", json!({})).await;
    let copy_id: kimchi_core::Id = copy["id"].as_str().unwrap().parse().unwrap();
    ok(&s, Source::Window, "project.delete", json!({ "projectId": orig.to_string() })).await;
    let p = s.library.load(copy_id).unwrap();
    let path = std::path::Path::new(&p.assets[0].path);
    assert!(path.starts_with(s.library.project_dir(copy_id)) && path.is_file(), "{path:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn captions_files_in_other_encodings() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({})).await;
    let srt = dir.path().join("utf16.srt");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend("1\r\n00:00:01,000 --> 00:00:02,000\r\nÇa va\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nOui\r\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&srt, bytes).unwrap();
    assert_eq!(ok(&s, Source::Agent, "captions.import", json!({ "path": srt })).await["captions"], 2);
    let list = ok(&s, Source::Agent, "captions.list", json!({})).await;
    assert_eq!(list[0]["text"], "Ça va");
}

#[tokio::test]
async fn paths_and_lines_are_bounded() {
    let p = crate::commands::media::absolute("-take2.mov").unwrap();
    assert!(p.is_absolute() && p.ends_with("-take2.mov"));
    let mut r: &[u8] = b"short\r\nwaaaaaaaay too long\n";
    assert_eq!(crate::bridge::read_line(&mut r, 8).await.unwrap().as_deref(), Some("short"));
    assert!(crate::bridge::read_line(&mut r, 8).await.is_err());
}

/// The Studio's commands: stacks, expressions, materials, compositions, moving and copying,
/// keyframe shifts, views and renders ahead.
#[tokio::test(flavor = "multi_thread")]
async fn studio_commands() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Window, "project.create", json!({ "name": "Studio", "width": 640, "height": 360 })).await;

    // 2D: an effect stack, an expression, a composition, precompose, reorder, duplicate.
    let added = ok(&s, Source::Agent, "motion.add", json!({ "start": 0, "duration": 2, "scene": { "layers": [
        { "id": "a", "type": "rect", "width": 100, "height": 100, "fill": "#ff5a36", "keyframes": { "x": [[0, 0], [1, 100]] } },
        { "id": "b", "type": "ellipse", "fill": "#ffffff" }
    ] } })).await;
    let clip = added["clips"][0]["id"].as_str().unwrap().to_string();
    let r = ok(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": clip, "id": "a", "field": "effects", "item": { "type": "glow", "radius": 30 } })).await;
    assert_eq!(r["itemId"], "glow");
    ok(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": clip, "id": "a", "field": "effects", "item": { "type": "blur" }, "index": 0 })).await;
    let a = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "a" })).await;
    assert_eq!(a["effects"][0]["type"], "blur");
    ok(&s, Source::Agent, "motion.setKeyframes", json!({ "clipId": clip, "id": "a", "property": "effects.glow.radius", "keyframes": [[0, 0], [1, 40]] })).await;
    ok(&s, Source::Agent, "motion.moveStackItem", json!({ "clipId": clip, "id": "a", "field": "effects", "itemId": "blur", "index": 1 })).await;
    ok(&s, Source::Agent, "motion.removeStackItem", json!({ "clipId": clip, "id": "a", "field": "effects", "itemId": "glow" })).await;
    let a = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "a" })).await;
    assert_eq!(a["effects"].as_array().unwrap().len(), 1);
    assert!(a["keyframes"].get("effects.glow.radius").is_none(), "its keyframes went with it");
    let e = registry::call(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": clip, "id": "a", "field": "effects", "item": { "type": "blurr" } })).await.unwrap_err();
    assert!(e.contains("Did you mean `blur`"), "{e}");
    let e = registry::call(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": clip, "id": "a", "field": "efects", "item": { "type": "blur" } })).await.unwrap_err();
    assert!(e.contains("Did you mean `effects`"), "{e}");
    ok(&s, Source::Agent, "motion.setExpression", json!({ "clipId": clip, "id": "b", "property": "rotation", "expression": "time * 90" })).await;
    let e = registry::call(&s, Source::Agent, "motion.setExpression", json!({ "clipId": clip, "id": "b", "property": "rotation", "expression": "wigle(1, 2)" })).await.unwrap_err();
    assert!(e.contains("wiggle"), "{e}");
    ok(&s, Source::Agent, "motion.precompose", json!({ "clipId": clip, "ids": ["a"], "compositionId": "card" })).await;
    let scene = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip })).await;
    assert_eq!(scene["scene"]["layers"][0]["type"], "comp");
    assert_eq!(scene["scene"]["compositions"][0]["layers"][0]["id"], "a");
    ok(&s, Source::Agent, "motion.setComposition", json!({ "clipId": clip, "composition": { "id": "card", "duration": 1.5 } })).await;
    let scene = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip })).await;
    assert_eq!(scene["scene"]["compositions"][0]["layers"][0]["id"], "a", "layers kept");
    assert!(registry::call(&s, Source::Agent, "motion.removeComposition", json!({ "clipId": clip, "compositionId": "card" })).await.is_err(), "still shown");
    ok(&s, Source::Agent, "motion.moveLayer", json!({ "clipId": clip, "id": "b", "index": 0 })).await;
    let d = ok(&s, Source::Agent, "motion.duplicateLayer", json!({ "clipId": clip, "id": "b" })).await;
    assert_eq!(d["id"], "b2");
    let scene = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip })).await;
    let ids: Vec<&str> = scene["scene"]["layers"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["b", "b2", "card"]);
    ok(&s, Source::Agent, "motion.shiftKeyframes", json!({ "clipId": clip, "id": "a", "by": 0.5 })).await;
    let a = ok(&s, Source::Agent, "motion.get", json!({ "clipId": clip, "id": "a" })).await;
    assert_eq!(a["keyframes"]["x"][0][0].as_f64().or(a["keyframes"]["x"][0]["time"].as_f64()), Some(0.5));

    // 3D: modifiers, constraints, shared materials, cameras.
    let three = ok(&s, Source::Agent, "motion.add", json!({ "start": 3, "scene": { "type": "3d", "objects": [
        { "id": "cube", "type": "box" }, { "id": "ball", "type": "sphere", "position": [2, 0, 0] }
    ] } })).await;
    let c3 = three["clips"][0]["id"].as_str().unwrap().to_string();
    ok(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": c3, "id": "cube", "field": "modifiers", "item": { "type": "array", "count": 4 } })).await;
    ok(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": c3, "id": "camera", "field": "constraints", "item": { "type": "lookAt", "target": "ball" } })).await;
    let e = registry::call(&s, Source::Agent, "motion.setStackItem", json!({ "clipId": c3, "id": "cube", "field": "modifiers", "item": { "type": "boolean", "object": "nope" } })).await.unwrap_err();
    assert!(e.contains("nope"), "{e}");
    ok(&s, Source::Agent, "motion.setMaterial", json!({ "clipId": c3, "material": { "id": "gold", "color": "#e8b04a", "metallic": 1 } })).await;
    ok(&s, Source::Agent, "motion.updateLayer", json!({ "clipId": c3, "id": "ball", "props": { "material": "gold" } })).await;
    assert_eq!(ok(&s, Source::Agent, "motion.get", json!({ "clipId": c3, "id": "ball" })).await["material"], "gold");
    ok(&s, Source::Agent, "motion.removeMaterial", json!({ "clipId": c3, "materialId": "gold" })).await;
    assert_eq!(ok(&s, Source::Agent, "motion.get", json!({ "clipId": c3, "id": "ball" })).await["material"]["color"], "#e8b04a", "kept as its own");
    ok(&s, Source::Agent, "motion.setLayer", json!({ "clipId": c3, "layer": { "type": "camera", "id": "close", "position": [0, 0, 3] } })).await;
    ok(&s, Source::Agent, "motion.updateLayer", json!({ "clipId": c3, "id": "scene", "props": { "activeCamera": "close" } })).await;
    ok(&s, Source::Agent, "motion.moveLayer", json!({ "clipId": c3, "id": "ball", "parent": "cube" })).await;
    let cube = ok(&s, Source::Agent, "motion.get", json!({ "clipId": c3, "id": "cube" })).await;
    assert_eq!(cube["children"][0]["id"], "ball");
    let types = ok(&s, Source::Agent, "motion.stackTypes", json!({ "family": "modifiers" })).await;
    assert!(types["modifiers"]["types"].as_array().unwrap().iter().any(|t| t["type"] == "subdivision"));

    // Modelling: a box becomes a mesh, its top is extruded, the array is baked in.
    let r = ok(&s, Source::Agent, "motion.convertToMesh", json!({ "clipId": c3, "id": "cube" })).await;
    assert_eq!(r["result"]["faces"], 6);
    let r = ok(&s, Source::Agent, "motion.editMesh", json!({ "clipId": c3, "id": "cube", "op": "extrude", "select": { "facing": [0, 1, 0] }, "params": { "distance": 0.5 } })).await;
    assert_eq!(r["result"]["faces"], 10, "{r}");
    assert!(!r["result"]["selection"]["faces"].as_array().unwrap().is_empty());
    let e = registry::call(&s, Source::Agent, "motion.editMesh", json!({ "clipId": c3, "id": "cube", "op": "extrud" })).await.unwrap_err();
    assert!(e.contains("extrude"), "{e}");
    let e = registry::call(&s, Source::Agent, "motion.editMesh", json!({ "clipId": c3, "id": "ball", "op": "extrude" })).await.unwrap_err();
    assert!(e.contains("convertToMesh"), "{e}");
    let r = ok(&s, Source::Agent, "motion.applyModifier", json!({ "clipId": c3, "id": "cube" })).await;
    assert_eq!(r["result"]["faces"], 40, "four copies: {r}");
    assert!(ok(&s, Source::Agent, "motion.get", json!({ "clipId": c3, "id": "cube" })).await.get("modifiers").is_none());
    // A model keeps its file's colours as a mesh.
    let obj = dir.path().join("red.obj");
    std::fs::write(dir.path().join("red.mtl"), "newmtl red\nKd 1 0 0\n").unwrap();
    std::fs::write(&obj, "mtllib red.mtl\nusemtl red\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 3 2\nf 1 2 4\nf 1 4 3\nf 2 3 4\n").unwrap();
    ok(&s, Source::Agent, "motion.setLayer", json!({ "clipId": c3, "layer": { "id": "toy", "type": "model", "src": obj.to_str().unwrap() } })).await;
    ok(&s, Source::Agent, "motion.convertToMesh", json!({ "clipId": c3, "id": "toy" })).await;
    let toy = ok(&s, Source::Agent, "motion.get", json!({ "clipId": c3, "id": "toy" })).await;
    assert_eq!((toy["type"].as_str(), toy["material"]["color"].as_str()), (Some("mesh"), Some("#ff0000")), "{toy}");

    // Looking and rendering ahead (needs ffmpeg).
    if s.tools().is_ok() {
        let v = ok(&s, Source::Agent, "motion.view", json!({ "clipId": c3, "axis": "top", "width": 320 })).await;
        assert!(std::path::Path::new(v["path"].as_str().unwrap()).is_file());
        let r = ok(&s, Source::Agent, "motion.render", json!({ "clipIds": [clip], "wait": true })).await;
        assert_eq!(r["renders"][0]["done"], true);
        let st = ok(&s, Source::Agent, "motion.renderStatus", json!({})).await;
        let state = st["clips"].as_array().unwrap().iter().find(|c| c["clipId"] == clip.as_str()).unwrap()["state"].clone();
        assert_eq!(state, "rendered");
        ok(&s, Source::Agent, "motion.updateLayer", json!({ "clipId": clip, "id": "b", "props": { "x": 10 } })).await;
        let st = ok(&s, Source::Agent, "motion.renderStatus", json!({})).await;
        let state = st["clips"].as_array().unwrap().iter().find(|c| c["clipId"] == clip.as_str()).unwrap()["state"].clone();
        assert_eq!(state, "outdated");
        ok(&s, Source::Agent, "motion.unrender", json!({ "clipIds": [clip] })).await;
        ok(&s, Source::Agent, "history.undo", json!({})).await;
        assert!(s.project().unwrap().clip(clip.parse().unwrap()).unwrap().rendered.is_some(), "undo brings the render back");
    }
}

/// A cut from a script: clip.delete hands back the clips as they were, and clip.paste takes them.
#[tokio::test(flavor = "multi_thread")]
async fn deleted_clips_can_be_pasted_back() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    ok(&s, Source::Cli, "clip.addText", json!({ "text": "Cut me", "start": 1, "duration": 2 })).await;
    let cut = ok(&s, Source::Cli, "clip.delete", json!({ "clipIds": ["Cut me"] })).await;
    let removed = cut["removed"].as_array().unwrap();
    assert_eq!(removed.len(), 1);
    assert!(removed[0]["trackId"].is_string(), "{cut}");
    assert_eq!(s.project().unwrap().clips().count(), 0);
    let pasted = ok(&s, Source::Cli, "clip.paste", json!({ "clips": removed, "time": 5 })).await;
    assert_eq!(pasted["clips"].as_array().unwrap().len(), 1, "{pasted}");
    let p = s.project().unwrap();
    let (_, c) = p.clips().next().unwrap();
    assert_eq!((c.start, c.duration), (5.0, 2.0));
}

/// What `ui.action` runs as the window is held to the permission of what it does.
#[tokio::test(flavor = "multi_thread")]
async fn window_actions_are_named_and_checked() {
    use futures::StreamExt;
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    // A stand-in window that answers every call with what it was asked.
    let mut calls = s.attach_ui();
    tokio::spawn(async move {
        while let Some(c) = calls.next().await {
            let _ = c.reply.send(Ok(json!({ "command": c.command, "params": c.params })));
        }
    });
    let spec = registry::spec("ui.action").unwrap();
    for (name, _) in crate::commands::ui::ACTIONS {
        assert!(spec.params[0].doc.contains(name), "{name} is missing from ui.action's documentation");
    }
    let v = ok(&s, Source::Mcp, "ui.action", json!({ "action": "pasteclips" })).await;
    assert_eq!(v["params"]["action"], "PasteClips", "names are matched without case");
    let e = registry::call(&s, Source::Cli, "ui.action", json!({ "action": "PasteClip" })).await.unwrap_err();
    assert!(e.contains("Did you mean PasteClips"), "{e}");
    // Settings are off for agents by default: the theme toggle is refused, the person's CLI isn't.
    let e = registry::call(&s, Source::Agent, "ui.action", json!({ "action": "ToggleTheme" })).await.unwrap_err();
    assert!(e.contains("\"settings\" permission"), "{e}");
    ok(&s, Source::Cli, "ui.action", json!({ "action": "ToggleTheme" })).await;
    let e = registry::call(&s, Source::Mcp, "ui.action", json!({ "action": "Quit" })).await.unwrap_err();
    assert!(e.contains("app control"), "{e}");

    let e = registry::call(&s, Source::Cli, "timeline.play", json!({ "speed": 0 })).await.unwrap_err();
    assert!(e.contains("-8 to 8"), "{e}");
    ok(&s, Source::Cli, "timeline.play", json!({ "speed": -2 })).await;
    let e = registry::call(&s, Source::Cli, "ui.reveal", json!({ "path": dir.path().join("nope") })).await.unwrap_err();
    assert!(e.contains("doesn't exist"), "{e}");
    // The built-in agent lives in the app: without it, agent.* says so.
    let e = registry::call(&s, Source::Cli, "agent.runs", json!({})).await.unwrap_err();
    assert!(e.contains("built-in agent runs"), "{e}");
}

// ---- audio -----------------------------------------------------------------------------------

/// A sine tone as a WAV file (the test is skipped without ffmpeg).
fn tone(dir: &std::path::Path, name: &str, hz: u32, seconds: u32) -> Option<std::path::PathBuf> {
    let tools = kimchi_media::Tools::locate().ok()?;
    let out = dir.join(name);
    let ok = std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", &format!("sine=frequency={hz}:duration={seconds}")]).arg(&out).status().ok()?.success();
    ok.then_some(out)
}

fn undo_steps(s: &Session) -> usize {
    s.read(|ed| ed.undo_steps().len()).unwrap()
}

/// Tracks, clips, buses, sends and the master: every edit one undo step, names everywhere,
/// mistakes explained.
#[tokio::test(flavor = "multi_thread")]
async fn audio_mix_tracks_clips_buses_and_master() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({ "name": "Mix" })).await;
    let Some(wav) = tone(dir.path(), "voice.wav", 440, 4) else { return eprintln!("ffmpeg not found; skipping") };
    ok(&s, Source::Cli, "media.import", json!({ "paths": [wav], "place": true, "start": 0, "trackId": "Audio 1" })).await;

    // Track: fader, pan, solo; a drag is one step; mistakes are explained.
    let before = undo_steps(&s);
    ok(&s, Source::Agent, "audio.setTrack", json!({ "trackId": "audio 1", "gainDb": -3, "coalesce": "drag" })).await;
    let t = ok(&s, Source::Agent, "audio.setTrack", json!({ "trackId": "Audio 1", "gainDb": -6, "pan": 0.25, "coalesce": "drag" })).await;
    assert_eq!((t["gainDb"].as_f64(), t["pan"].as_f64()), (Some(-6.0), Some(0.25)), "{t}");
    assert_eq!(undo_steps(&s), before + 1, "a drag is one undo step");
    let e = registry::call(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Audio 1", "gainDb": 20 })).await.unwrap_err();
    assert!(e.contains("+12 dB"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Audio 1", "pan": 2 })).await.unwrap_err();
    assert!(e.contains("-1 (left)"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Audoi 1", "gainDb": 0 })).await.unwrap_err();
    assert!(e.contains("Did you mean Audio 1"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Video 1", "armed": true })).await.unwrap_err();
    assert!(e.contains("audio tracks"), "{e}");
    ok(&s, Source::Window, "history.undo", json!({})).await;
    assert_eq!(s.project().unwrap().tracks[1].mix.gain_db, 0.0, "undo puts the fader back");

    // Clips: gain in dB, fade shape, channels; only clips with sound.
    let c = ok(&s, Source::Cli, "audio.setClip", json!({ "clipIds": ["voice.wav"], "gainDb": -6, "fadeCurve": "equal power", "channels": "mono", "pitch": 2, "preservePitch": false })).await;
    assert!((c[0]["volume"].as_f64().unwrap() - 0.501).abs() < 1e-3, "{c}");
    assert_eq!(c[0]["fadeCurve"], "equalPower");
    assert_eq!(c[0]["channels"], "mono");
    let e = registry::call(&s, Source::Cli, "audio.setClip", json!({ "clipIds": ["voice.wav"], "fadeCurve": "logarithmic" })).await.unwrap_err();
    assert!(e.contains("linear, equalPower"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.setClip", json!({ "clipIds": ["voice.wav"], "pitch": 30 })).await.unwrap_err();
    assert!(e.contains("semitones"), "{e}");
    ok(&s, Source::Cli, "clip.addText", json!({ "text": "Title", "start": 0 })).await;
    let e = registry::call(&s, Source::Cli, "audio.setClip", json!({ "clipIds": ["Title"], "gainDb": 0 })).await.unwrap_err();
    assert!(e.contains("has no sound"), "{e}");
    // Clip pan animates like any clip property.
    ok(&s, Source::Cli, "clip.setKeyframes", json!({ "clipId": "voice.wav", "property": "pan", "keyframes": [[0, -1], [2, 1]] })).await;
    let p = s.project().unwrap();
    let clip = p.clips().find(|(_, c)| c.name == "voice.wav").unwrap().1;
    assert!((clip.pan_at(1.0)).abs() < 1e-9);

    // A reverb bus, a send to it, the track feeding it; removing it routes back to the master.
    let bus = ok(&s, Source::Cli, "audio.addBus", json!({ "name": "Reverb", "effect": "space" })).await;
    assert_eq!(bus["effects"][0]["effect"], "Space", "{bus}");
    let t = ok(&s, Source::Cli, "audio.setSend", json!({ "trackId": "Audio 1", "busId": "reverb", "levelDb": -12 })).await;
    assert_eq!(t["sends"][0]["levelDb"], -12.0, "{t}");
    let e = registry::call(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Audio 1", "output": "Reverbb" })).await;
    assert!(e.unwrap_err().contains("Did you mean Reverb"));
    ok(&s, Source::Cli, "audio.addBus", json!({ "name": "Dialogue" })).await;
    let t = ok(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Audio 1", "output": "Dialogue" })).await;
    assert_eq!(t["output"], "Dialogue");
    ok(&s, Source::Cli, "audio.setBus", json!({ "busId": "Dialogue", "gainDb": -2, "name": "Voices" })).await;
    ok(&s, Source::Cli, "audio.removeBus", json!({ "busId": "Voices" })).await;
    let p = s.project().unwrap();
    assert_eq!(p.tracks[1].mix.output, None, "a track whose bus is gone goes to the master");
    ok(&s, Source::Cli, "audio.removeSend", json!({ "trackId": "Audio 1", "busId": "Reverb" })).await;
    assert!(s.project().unwrap().tracks[1].mix.sends.is_empty());

    // The master: loudness by name, off again, the limiter's ceiling checked.
    let m = ok(&s, Source::Cli, "audio.setMaster", json!({ "loudness": "podcast", "ceilingDb": -2 })).await;
    assert_eq!((m["loudness"].as_f64(), m["loudnessFor"].as_str()), (Some(-16.0), Some("podcast")), "{m}");
    let m = ok(&s, Source::Cli, "audio.setMaster", json!({ "loudness": null })).await;
    assert!(m["loudness"].is_null(), "{m}");
    let e = registry::call(&s, Source::Cli, "audio.setMaster", json!({ "ceilingDb": 3 })).await.unwrap_err();
    assert!(e.contains("ceiling"), "{e}");

    // The overview reads it all back, with what may surprise.
    ok(&s, Source::Cli, "audio.setTrack", json!({ "trackId": "Audio 1", "solo": true })).await;
    let o = ok(&s, Source::Mcp, "audio.overview", json!({})).await;
    assert_eq!(o["buses"][0]["name"], "Reverb", "{o}");
    assert!(o["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("Solo is on")), "{o}");
    assert!(o["problems"].as_array().unwrap().iter().any(|p| p.as_str().unwrap().contains("Nothing feeds the bus Reverb")), "{o}");
    assert_eq!(o["clips"][0]["channels"], "mono", "{o}");
}

/// Effect chains in ryolune's insert format: add by name with typed values, change, bypass,
/// move, presets, copy, remove; automation of faders and parameters.
#[tokio::test(flavor = "multi_thread")]
async fn audio_effects_and_automation() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    let list = ok(&s, Source::Mcp, "audio.effects", json!({ "query": "eq" })).await;
    assert!(list.as_array().unwrap().iter().any(|e| e["id"] == "stock:Channel EQ"), "{list}");
    let params = ok(&s, Source::Mcp, "audio.effectParams", json!({ "effect": "Channel EQ" })).await;
    assert_eq!(params["params"][0]["name"], "Low Gain", "{params}");

    let fx = ok(&s, Source::Agent, "audio.addEffect", json!({ "target": "Audio 1", "effect": "channel eq", "params": { "low gain": "-6 dB", "High Freq": "8k" } })).await;
    assert!(fx["params"]["Low Gain"].as_str().unwrap().starts_with("-6.0"), "{fx}");
    assert!(fx["params"]["High Freq"].as_str().unwrap().starts_with("8000"), "{fx}");
    let slot = fx["slot"].as_str().unwrap().to_string();
    let e = registry::call(&s, Source::Cli, "audio.addEffect", json!({ "target": "Audio 1", "effect": "Chanel EQ" })).await.unwrap_err();
    assert!(e.contains("Did you mean `Channel EQ`"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.addEffect", json!({ "target": "Audio 1", "effect": "Space", "params": { "Sise": 50 } })).await.unwrap_err();
    assert!(e.contains("Did you mean `Size`"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.addEffect", json!({ "target": "Audio 1", "effect": "Space", "params": { "Mix": 300 } })).await.unwrap_err();
    assert!(e.contains("goes from"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.addEffect", json!({ "target": "Nowhere", "effect": "Space" })).await.unwrap_err();
    assert!(e.contains("No track, bus or clip"), "{e}");

    ok(&s, Source::Cli, "audio.addEffect", json!({ "target": "track:Audio 1", "effect": "Space", "index": 0 })).await;
    let chain = || s.project().unwrap().tracks[1].mix.effects.clone();
    assert_eq!(chain()[0].name, "Space");
    ok(&s, Source::Cli, "audio.moveEffect", json!({ "target": "Audio 1", "slot": "Space", "index": 1 })).await;
    assert_eq!(chain()[1].name, "Space");
    let v = ok(&s, Source::Cli, "audio.setEffect", json!({ "target": "Audio 1", "slot": 2, "params": { "Mix": 45 }, "bypassed": true })).await;
    assert!(v["params"]["Mix"].as_str().unwrap().starts_with("45") && v["bypassed"] == true, "{v}");
    let v = ok(&s, Source::Cli, "audio.applyPreset", json!({ "target": "Audio 1", "slot": slot, "preset": "telephone" })).await;
    assert!(v["params"]["Low Gain"].as_str().unwrap().starts_with("-15.0"), "{v}");
    let e = registry::call(&s, Source::Cli, "audio.applyPreset", json!({ "target": "Audio 1", "slot": "Space", "preset": "Cathedrall" })).await.unwrap_err();
    assert!(e.contains("Did you mean `Cathedral`"), "{e}");
    let presets = ok(&s, Source::Cli, "audio.effectPresets", json!({ "effect": "Space" })).await;
    assert!(presets.as_array().unwrap().iter().any(|p| p["preset"] == "Small room"), "{presets}");

    // The chain goes to the master and a bus as one step.
    ok(&s, Source::Cli, "audio.addBus", json!({ "name": "Group" })).await;
    let before = undo_steps(&s);
    ok(&s, Source::Cli, "audio.copyEffects", json!({ "from": "Audio 1", "to": ["master", "Group"] })).await;
    assert_eq!(undo_steps(&s), before + 1);
    let p = s.project().unwrap();
    assert_eq!(p.mixer.master.effects.len(), 2);
    assert_eq!(p.mixer.buses[0].mix.effects.len(), 2);

    // Automation: the fader over time, a parameter by its readable name.
    ok(&s, Source::Cli, "audio.setAutomation", json!({ "target": "Audio 1", "property": "gainDb", "keyframes": [[0, -12], [2, 0, "easeOut"]] })).await;
    let mix = s.project().unwrap().tracks[1].mix.clone();
    assert!((mix.gain_db_at(0.0) + 12.0).abs() < 1e-9 && mix.gain_db_at(3.0).abs() < 1e-9);
    let v = ok(&s, Source::Cli, "audio.addAutomationKey", json!({ "target": "Audio 1", "property": "Space.Mix", "time": 1, "value": "60%" })).await;
    let key = v["property"].as_str().unwrap().to_string();
    assert!(key.starts_with("effects.") && key.ends_with(".4"), "{v}");
    // An automated parameter set by value gets a keyframe at the playhead instead.
    ok(&s, Source::Cli, "audio.setEffect", json!({ "target": "Audio 1", "slot": "Space", "params": { "Mix": 20 } })).await;
    let mix = s.project().unwrap().tracks[1].mix.clone();
    assert_eq!(mix.keyframes[&key].len(), 2, "{:?}", mix.keyframes);
    let e = registry::call(&s, Source::Cli, "audio.setAutomation", json!({ "target": "master", "property": "pan", "keyframes": [[0, 0]] })).await.unwrap_err();
    assert!(e.contains("master has no pan"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.addAutomationKey", json!({ "target": "Audio 1", "property": "Space.Wetness" })).await.unwrap_err();
    assert!(e.contains("Did you mean") || e.contains("no parameter"), "{e}");
    ok(&s, Source::Cli, "audio.removeAutomationKey", json!({ "target": "Audio 1", "property": "gainDb" })).await;
    assert!(!s.project().unwrap().tracks[1].mix.keyframes.contains_key("gainDb"));
    // Removing an effect takes its automation with it.
    ok(&s, Source::Cli, "audio.removeEffect", json!({ "target": "Audio 1", "slot": "Space" })).await;
    assert!(s.project().unwrap().tracks[1].mix.keyframes.is_empty());
}

/// Ducking the music under the dialogue, guessed from the tracks; cutting on beats.
#[tokio::test(flavor = "multi_thread")]
async fn audio_ducking_and_beat_cuts() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    let (Some(music), Some(voice)) = (tone(dir.path(), "song.wav", 220, 8), tone(dir.path(), "take.wav", 440, 4)) else { return eprintln!("ffmpeg not found; skipping") };
    ok(&s, Source::Cli, "track.update", json!({ "trackId": "Audio 1", "name": "Music" })).await;
    ok(&s, Source::Cli, "track.add", json!({ "kind": "audio" })).await;
    ok(&s, Source::Cli, "track.update", json!({ "trackId": "Audio 2", "name": "Voice" })).await;
    ok(&s, Source::Cli, "media.import", json!({ "paths": [music], "place": true, "start": 0, "trackId": "Music" })).await;
    ok(&s, Source::Cli, "media.import", json!({ "paths": [voice], "place": true, "start": 1, "trackId": "Voice" })).await;
    let d = ok(&s, Source::Agent, "audio.autoDuck", json!({ "amountDb": 10 })).await;
    assert_eq!((d["music"].clone(), d["dialogue"].clone()), (json!(["Music"]), json!(["Voice"])), "{d}");
    let p = s.project().unwrap();
    let duck = p.tracks.iter().find(|t| t.name == "Music").unwrap().mix.duck.clone().unwrap();
    assert_eq!(duck.amount_db, -10.0);
    assert_eq!(duck.under, vec![p.tracks.iter().find(|t| t.name == "Voice").unwrap().id]);
    ok(&s, Source::Cli, "audio.autoDuck", json!({ "off": true, "music": ["Music"] })).await;
    assert!(s.project().unwrap().tracks.iter().all(|t| t.mix.duck.is_none()));

    // Beats stored with the music (as detection or a song would), then cut on every bar.
    let p = s.project().unwrap();
    let mut asset = p.assets.iter().find(|a| a.name == "song.wav").unwrap().clone();
    asset.beats = Some(kimchi_core::Beats { tempo: 120.0, beats_per_bar: 4, times: (0..16).map(|i| i as f64 * 0.5).collect(), first_downbeat: 0, source: "detected".into() });
    s.apply("test", Source::Cli, &kimchi_core::Edit::UpdateAsset { asset }, None).unwrap();
    ok(&s, Source::Cli, "clip.addSolid", json!({ "color": "#202020", "start": 0, "duration": 8, "trackId": "Video 1" })).await;
    let before = undo_steps(&s);
    let cut = ok(&s, Source::Agent, "audio.beatCut", json!({ "musicClipId": "song.wav", "every": 4 })).await;
    // Bars at 0, 2, 4, 6 s: the clip is cut at 2, 4 and 6 (0 is its start).
    assert_eq!(cut["cuts"], 3, "{cut}");
    assert_eq!(undo_steps(&s), before + 1);
    let p = s.project().unwrap();
    let starts: Vec<f64> = p.tracks[0].clips.iter().map(|c| c.start).collect();
    assert_eq!(starts, vec![0.0, 2.0, 4.0, 6.0]);
    let m = ok(&s, Source::Cli, "audio.beatCut", json!({ "musicClipId": "song.wav", "every": 8, "markers": true })).await;
    assert_eq!(m["markers"], 2, "{m}");
    let o = ok(&s, Source::Cli, "audio.overview", json!({})).await;
    assert_eq!(o["beats"][0]["tempo"], 120.0, "{o}");
}

/// Window-only audio commands need the window; settings take audio values.
#[tokio::test(flavor = "multi_thread")]
async fn audio_live_commands_and_settings() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    for name in ["audio.meters", "audio.devices", "audio.showMixer"] {
        let e = registry::call(&s, Source::Cli, name, json!({})).await.unwrap_err();
        assert!(e.contains("needs the kimchi window"), "{name}: {e}");
    }
    let e = registry::call(&s, Source::Cli, "audio.record", json!({ "action": "start" })).await.unwrap_err();
    assert!(e.contains("needs the kimchi window"), "{e}");
    ok(&s, Source::Cli, "audio.scrub", json!({ "on": false })).await;
    assert!(!s.settings().audio.scrub);
    ok(&s, Source::Cli, "app.setSetting", json!({ "key": "audio.pluginFolders", "value": ["/opt/plugins"] })).await;
    assert_eq!(s.settings().audio.plugin_folders, vec!["/opt/plugins".to_string()]);
    let e = registry::call(&s, Source::Cli, "app.setSetting", json!({ "key": "audio.defaultLoudness", "value": 3 })).await.unwrap_err();
    assert!(e.contains("-40 to -5"), "{e}");
    // A song that isn't a song.
    let e = registry::call(&s, Source::Cli, "audio.importSong", json!({ "path": dir.path().join("a.wav") })).await.unwrap_err();
    assert!(e.contains("isn't a ryolune song"), "{e}");
    let e = registry::call(&s, Source::Cli, "audio.openInRyolune", json!({})).await.unwrap_err();
    assert!(e.contains("clipId"), "{e}");
}

/// Loudness through the mixer: a quiet tone measured, normalized to a target, measured again.
#[tokio::test(flavor = "multi_thread")]
async fn audio_loudness_measure_and_normalize() {
    let dir = tempfile::tempdir().unwrap();
    let s = session(dir.path());
    ok(&s, Source::Cli, "project.create", json!({})).await;
    let Ok(tools) = kimchi_media::Tools::locate() else { return eprintln!("ffmpeg not found; skipping") };
    let wav = dir.path().join("quiet.wav");
    std::process::Command::new(&tools.ffmpeg).args(["-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=1000:duration=4,volume=0.5"]).arg(&wav).status().unwrap();
    ok(&s, Source::Cli, "media.import", json!({ "paths": [wav], "place": true, "start": 0, "trackId": "Audio 1" })).await;
    let before = ok(&s, Source::Mcp, "audio.measure", json!({ "clipId": "quiet.wav" })).await;
    let loud = before["integrated"].as_f64().unwrap();
    assert!(loud < -20.0 && loud > -40.0, "{before}");
    let n = ok(&s, Source::Agent, "audio.normalize", json!({ "clipIds": ["quiet.wav"], "target": -16 })).await;
    assert!((n["clips"][0]["gainDb"].as_f64().unwrap() - (-16.0 - loud)).abs() < 0.2, "{n}");
    let after = ok(&s, Source::Mcp, "audio.measure", json!({})).await;
    assert!((after["integrated"].as_f64().unwrap() + 16.0).abs() < 1.5, "{after}");
    let e = registry::call(&s, Source::Cli, "audio.normalize", json!({ "clipIds": ["quiet.wav"], "mode": "peak", "target": 3 })).await.unwrap_err();
    assert!(e.contains("Peak targets"), "{e}");
}

/// The plugin recipe of lsuite's PLUGINS.md, end to end: scaffold from the SDK's template, a
/// build with an error reported as data, a green build, publish (bundle, install, load), the
/// plugin on a clip in a rendered frame, a rebuilt version used at once (hot reload), the switch,
/// and removal. Needs cargo (skipped without it); builds share `target/plugin-tests`.
#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_is_made_built_installed_used_and_reloaded() {
    if crate::plugin_dev::toolchain().await.cargo.is_none() {
        eprintln!("no cargo: skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let _env = crate::account::testing(&dir.path().join("lsuite"), None);
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    crate::plugin_dev::set_target_dir_for_tests(workspace.join("target/plugin-tests"));
    let s = session(dir.path());
    crate::session::configure_plugins(&s);

    // Agents need the plugins permission (off by default).
    let e = registry::call(&s, Source::Agent, "plugin.new", json!({ "name": "warm-test" })).await.unwrap_err();
    assert!(e.contains("\"plugins\" permission"), "{e}");
    s.update_settings(|st| st.agent.permissions.plugins = true).unwrap();

    let list = ok(&s, Source::Agent, "plugin.list", json!({})).await;
    let stock: Vec<&str> = list["plugins"].as_array().unwrap().iter().filter(|p| p["source"] == "stock").filter_map(|p| p["name"].as_str()).collect();
    for want in ["Brightness", "Dissolve", "Punchy", "Halftone", "Radial wipe"] {
        assert!(stock.contains(&want), "{want} in {stock:?}");
    }
    let formats: Vec<&str> = list["formats"].as_array().unwrap().iter().filter_map(|f| f["id"].as_str()).collect();
    assert!(formats.starts_with(&["lsuite", "frei0r", "lut", "ryolune", "clap", "vst3"]), "{formats:?}");
    assert!(ok(&s, Source::Agent, "plugin.guide", json!({})).await["guide"].as_str().unwrap().contains("plugin.publishLocal"));

    let made = ok(&s, Source::Agent, "plugin.new", json!({ "name": "warm-test", "kind": "effect", "description": "Warms the picture." })).await;
    assert_eq!(made["id"], "local.plugins.warm-test");
    assert!(made["source"].as_str().unwrap().contains("pub struct WarmTest") && made["source"].as_str().unwrap().contains("export_plugins!(WarmTest)"));
    let e = registry::call(&s, Source::Agent, "plugin.new", json!({ "name": "warm-test" })).await.unwrap_err();
    assert!(e.contains("exists already"), "{e}");
    let e = registry::call(&s, Source::Agent, "plugin.writeSource", json!({ "name": "warm-test", "path": "../escape.rs", "contents": "" })).await.unwrap_err();
    assert!(e.contains("inside the crate"), "{e}");

    // An error comes back as data, with its place.
    let src = made["source"].as_str().unwrap().to_string();
    ok(&s, Source::Agent, "plugin.writeSource", json!({ "name": "warm-test", "path": "src/lib.rs", "contents": src.replace("let input = inputs[0];", "let input = inputs[0]; let oops: u8 = \"no\";") })).await;
    let b = ok(&s, Source::Agent, "plugin.build", json!({ "name": "warm-test" })).await;
    assert_eq!(b["ok"], false, "{b}");
    let first = &b["errors"][0];
    assert_eq!(first["file"], "src/lib.rs", "{b}");
    assert!(first["line"].as_u64().unwrap() > 10 && first["message"].as_str().unwrap().contains("mismatched types"), "{b}");

    // Fixed, built, published: a solid clip turns the template's orange.
    ok(&s, Source::Agent, "plugin.writeSource", json!({ "name": "warm-test", "path": "src/lib.rs", "contents": src })).await;
    let p = ok(&s, Source::Agent, "plugin.publishLocal", json!({ "name": "warm-test" })).await;
    assert_eq!(p["published"], true, "{p}");
    assert_eq!(p["plugins"][0]["id"], "kimchi:local.plugins.warm-test");
    assert!(dir.path().join("lsuite/plugins/kimchi/local.plugins.warm-test/plugin.toml").is_file());
    let installed = ok(&s, Source::Agent, "plugin.list", json!({ "source": "installed", "kind": "effect" })).await;
    assert_eq!(installed["plugins"][0]["name"], "Warm test");
    assert_eq!(installed["plugins"][0]["enabled"], true);

    ok(&s, Source::Window, "project.create", json!({ "name": "P", "width": 320, "height": 180 })).await;
    let solid = ok(&s, Source::Agent, "clip.addSolid", json!({ "color": "#000000", "duration": 2 })).await;
    let clip = solid["clips"][0]["id"].as_str().map(str::to_string).or_else(|| solid["id"].as_str().map(str::to_string)).unwrap();
    ok(&s, Source::Agent, "clip.addPlugin", json!({ "clipIds": [clip], "plugin": "Warm test", "params": { "Amount": 100 } })).await;
    let pixel = |s: Arc<Session>| async move {
        let f = ok(&s, Source::Agent, "project.renderFrame", json!({ "time": 1.0, "width": 160 })).await;
        let png = kimchi_media::tiny_skia::Pixmap::load_png(f["path"].as_str().unwrap()).unwrap();
        let p = png.pixel(80, 45).unwrap();
        (p.red(), p.green(), p.blue())
    };
    let (r, g, b) = pixel(s.clone()).await;
    assert!(r > 240 && (130..150).contains(&g) && (40..60).contains(&b), "orange: {r} {g} {b}");

    // Rebuilt to tint blue: the next frame uses the new build, without a restart.
    let blue = made["source"].as_str().unwrap().replace("[1.0, 0.55, 0.2, 1.0]", "[0.0, 0.2, 1.0, 1.0]");
    ok(&s, Source::Agent, "plugin.writeSource", json!({ "name": "warm-test", "path": "src/lib.rs", "contents": blue })).await;
    ok(&s, Source::Agent, "plugin.publishLocal", json!({ "name": "warm-test" })).await;
    // The clip keeps the value it was given (the default changed, not the clip's).
    ok(&s, Source::Agent, "clip.setPlugin", json!({ "clipId": clip, "slot": "p1", "params": { "Colour": "#0033ff" } })).await;
    let (r, _, b) = pixel(s.clone()).await;
    assert!(r < 20 && b > 240, "blue now: {r} {b}");

    // Switched off: drawn without it. On again: back.
    ok(&s, Source::Agent, "plugin.disable", json!({ "id": "Warm test" })).await;
    assert_eq!(pixel(s.clone()).await, (0, 0, 0));
    ok(&s, Source::Agent, "plugin.enable", json!({ "id": "Warm test" })).await;
    assert!(pixel(s.clone()).await.2 > 240);

    // Removed: the clip keeps its settings and is drawn without it.
    ok(&s, Source::Agent, "plugin.remove", json!({ "id": "local.plugins.warm-test" })).await;
    assert!(!dir.path().join("lsuite/plugins/kimchi/local.plugins.warm-test").exists());
    assert_eq!(pixel(s.clone()).await, (0, 0, 0));
    let c = ok(&s, Source::Agent, "clip.get", json!({ "clipId": clip })).await;
    assert!(c.to_string().contains("local.plugins.warm-test"), "{c}");
}
