use super::*;
use gpui::point;
use crate::tests::{setup, store_settles};

#[gpui::test]
fn space_geometry_uses_clip_expressions_for_components_helpers_and_framing(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    f.call("project.setSettings",json!({"fps":60}));
    let added=f.call("motion.add",json!({"duration":8,"scene":{"type":"3d",
        "camera":{"position":[0,0,8],"target":[0,0,0],"projection":"orthographic","orthoSize":6,
            "expressions":{"position.x":"duration * 0.1","target.x":"duration * 0.1"}},
        "lights":[{"id":"light","type":"point","expressions":{"position.x":"duration * 0.2"}}],
        "objects":[{"id":"panel","type":"mesh","vertices":[[-0.5,-0.5,0],[0.5,-0.5,0],[0.5,0.5,0],[-0.5,0.5,0]],"faces":[[0,1,2,3]],
            "expressions":{"position.x":"duration * 0.3","position.y":"fps / 60 - 1"}}]
    }}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["panel".into()],cx);s.tool=Tool::Select;
        s.view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho:true,ortho_size:6.,..Default::default()};
        s.set_mode(Mode::Edit,cx).unwrap();s.set_select_mode(SelectMode::Face,cx);s.changed(cx);
    }));cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    let point=cx.update(|_,cx| {
        let vp=viewport.read(cx);let (vertices,_,view)=vp.mesh(cx).unwrap();
        assert!((vertices[0][0]-1.9).abs()<1e-9 && vertices[0][1]==-0.5,"evaluated mesh position: {:?}",vertices[0]);
        let (_,_,Scene::Space(scene),time)=vp.scene(cx).unwrap() else {panic!()};
        let worlds=studio.read(cx).worlds(&scene,time,cx);
        assert!((math::origin(&worlds["light"])[0]-1.6).abs()<1e-9);
        assert!((math::origin(&worlds["camera"])[0]-0.8).abs()<1e-9);
        view.project([2.4,0.,0.]).unwrap()
    });
    cx.update(|_,cx|viewport.update(cx,|v,cx|v.mesh_click(point,false,cx)));
    assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.faces.clone()),vec![0]);
    cx.update(|_,cx|studio.update(cx,|s,cx| {s.set_mode(Mode::Object,cx).unwrap();s.set_selection(vec![],cx);}));
    cx.update(|_,cx|viewport.update(cx,|v,cx|v.click(point,false,cx)));
    assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),vec!["panel"]);
    cx.update(|_,cx|studio.update(cx,|s,cx|s.frame_selection(false,cx)));cx.run_until_parked();
    assert!((cx.update(|_,cx|studio.read(cx).view.target[0])-2.4).abs()<1e-5);
    cx.update(|_,cx|studio.update(cx,|s,cx| {s.through_camera=true;s.changed(cx);}));cx.run_until_parked();
    cx.update(|_,cx| {
        let vp=viewport.read(cx);let (project,_,Scene::Space(scene),time)=vp.scene(cx).unwrap() else {panic!()};let view=vp.view3(&scene,time,&project,cx);
        assert!((view.cam.position[0]-0.8).abs()<1e-9 && (view.cam.target[0]-0.8).abs()<1e-9);
    });
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn canvas_geometry_keeps_picking_handles_and_framing_with_the_open_clip(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    f.call("project.setSettings",json!({"width":800,"height":600,"fps":60}));
    let added=f.call("motion.add",json!({"duration":8,"scene":{"layers":[
        {"id":"card","type":"rect","width":120,"height":60,"expressions":{"x":"duration * 20 + frame","y":"fps","anchorX":"fps / 10"}},
        {"id":"comp","type":"comp","comp":"nested","hidden":true}
    ],"compositions":[{"id":"nested","layers":[{"id":"inner","type":"rect"}]}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    let foreign=f.call("motion.add",json!({"start":10,"duration":1,"scene":{"layers":[{"id":"other","type":"rect"}]}}));
    let foreign:Id=foreign["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s|s.clip(foreign).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["card".into()],cx);s.tool=Tool::Select;
        s.canvas=super::super::Canvas2d {zoom:1.,pan:[0.;2],fit:false};s.changed(cx);
    }));cx.run_until_parked();
    let before=f.project();let history=f.call("history.list",json!({}));
    f.call("motion.view",json!({"clipId":foreign,"time":10,"width":160}));
    let centre=cx.update(|_,cx| {
        let v=viewport.read(cx);let (p,_,Scene::Flat(s),t)=v.scene(cx).unwrap() else {panic!()};
        let view=v.view2(&s,&p,cx);let geometry=studio.read(cx).canvas_geometry(&s,t,&p).unwrap();
        assert_eq!(geometry.bounds("comp").unwrap()[0],[-400.,-300.]);
        let (corners,_,anchor)=v.handles2(&geometry,&view,cx).unwrap();
        assert_eq!(corners[0],view.to_screen([94.,30.]));
        assert_eq!(anchor,view.to_screen([160.,60.]),"the anchor also uses its evaluated expression");
        view.to_screen([154.,60.])
    });
    cx.update(|_,cx|viewport.update(cx,|v,cx|v.click(centre,false,cx)));
    assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),vec!["card"]);
    cx.update(|_,cx|viewport.update(cx,|v,cx|v.box_select([centre[0]-10.,centre[1]-10.],[centre[0]+10.,centre[1]+10.],SelectionOp::Replace,cx)));
    assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),vec!["card"]);
    cx.update(|_,cx|studio.update(cx,|s,cx|s.frame_selection(false,cx)));cx.run_until_parked();
    cx.update(|_,cx| {
        let v=viewport.read(cx);let (p,_,Scene::Flat(s),_)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
        let centre=view.to_screen([154.,60.]);let expected=v.bounds_for_test().center();
        assert!((centre[0]-f32::from(expected.x) as f64).abs()<1e-5 && (centre[1]-f32::from(expected.y) as f64).abs()<1e-5);
    });
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn canvas_crops_render_at_viewport_resolution_and_keep_old_frames_registered(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[{"id":"card","type":"rect","width":120,"height":60}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"clipId":clip,"zoom":16,"pan":[-1600,800]}),w,cx).unwrap()));cx.run_until_parked();
    let before=f.project();let history=f.call("history.list",json!({}));
    let original=cx.update(|_,cx| {
        let vp=viewport.read(cx);let request=vp.request(cx).unwrap();let canvas=request.canvas.unwrap();
        assert_eq!(canvas.centre,[100.,-50.]);assert!(request.w<=2048 && request.h<=2048);
        let (p,_,Scene::Flat(scene),_)=vp.scene(cx).unwrap() else {panic!()};let view=vp.view2(&scene,&p,cx);let rect=view.picture_box(canvas,request.w,request.h);
        let b=vp.bounds_for_test();let expected=[f32::from(b.origin.x) as f64,f32::from(b.origin.y) as f64,f32::from(b.size.width) as f64,f32::from(b.size.height) as f64];
        for (a,b) in rect.into_iter().zip(expected) {assert!((a-b).abs()<1e-6);}
        request
    });
    cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"zoom":2,"pan":[30,40]}),w,cx).unwrap()));cx.run_until_parked();
    cx.update(|_,cx|viewport.update(cx,|vp,cx| {
        let next=vp.request(cx).unwrap();assert_eq!((next.w,next.h),(original.w,original.h),"zoom changes detail, not the number of output pixels");
        let image=||to_image(kimchi_media::tiny_skia::Pixmap::new(16,16).unwrap()).unwrap();
        assert!(vp.accept_picture(original.clone(),Ok(image()),cx));
        let old=vp.image_canvas.unwrap();
        assert!(vp.accept_picture(next.clone(),Err("render failed".into()),cx));
        assert_eq!(vp.image_canvas,Some(old),"a failed replacement must not relabel the old frame with the new crop");
        let (p,_,Scene::Flat(scene),_)=vp.scene(cx).unwrap() else {panic!()};let view=vp.view2(&scene,&p,cx);
        let rect=view.picture_box(old.0,old.1,old.2);let centre=view.to_screen([100.,-50.]);
        assert!((rect[0]+rect[2]/2.-centre[0]).abs()<1e-6 && (rect[1]+rect[3]/2.-centre[1]).abs()<1e-6);
        assert!(vp.accept_picture(next.clone(),Ok(image()),cx));assert_eq!(vp.image_canvas,Some((next.canvas.unwrap(),next.w,next.h)));
    }));
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn framing_3d_selection_fits_portrait_and_landscape_viewports(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"wide","type":"mesh","vertices":[[-8,-1,0],[8,-1,0],[8,1,0],[-8,1,0]],"faces":[[0,1,2,3]],"position":[3,2,-1]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    crate::tests::resize(cx,420.,1100.);cx.run_until_parked();
    cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"clipId":clip,"select":["wide"],"panel":"none","frame":true,
        "view":{"position":[3,2,9],"target":[3,2,-1],"fov":40,"ortho":true,"orthoSize":6}}),w,cx).unwrap()));cx.run_until_parked();
    cx.update(|_,cx| {
        let vp=viewport.read(cx);let (p,_,Scene::Space(scene),time)=vp.scene(cx).unwrap() else {panic!()};let view=vp.view3(&scene,time,&p,cx);
        assert!(view.project([-5.,1.,-1.]).unwrap()[0]>view.x+12.,"opening and framing before the first layout must use the final viewport size: {view:?}");
    });
    let before=f.project();let history=f.call("history.list",json!({}));
    for (width,height) in [(1400.,900.),(420.,1100.),(720.,1200.)] {
        crate::tests::resize(cx,width,height);cx.run_until_parked();
        for ortho in [true,false] {
            cx.update(|_,cx|studio.update(cx,|s,cx| {
                s.view=ViewCamera {position:[3.,2.,9.],target:[3.,2.,-1.],ortho,..Default::default()};
                s.frame_selection(false,cx);
            }));cx.run_until_parked();
            cx.update(|_,cx| {
                let vp=viewport.read(cx);let (p,_,Scene::Space(scene),time)=vp.scene(cx).unwrap() else {panic!()};let view=vp.view3(&scene,time,&p,cx);
                let (vertices,_)=model::edit_mesh(&scene,&model::worlds(&scene,time),"wide").unwrap();
                for vertex in vertices {
                    let q=view.project(vertex).unwrap();
                    assert!(q[0]>view.x+12. && q[0]<view.x+view.w-12. && q[1]>view.y+12. && q[1]<view.y+view.h-12.,"{width}×{height}, ortho={ortho}: {q:?} outside {view:?}");
                }
                assert_eq!(studio.read(cx).view.target,[3.,2.,-1.]);
            });
        }
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn framing_2d_layers_centres_visible_animated_bounds_and_repeated_instances(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[
        {"id":"card","type":"rect","width":120,"height":60,"rotation":30,"y":-80,"keyframes":{"x":[[0,0],[2,400]]}},
        {"id":"copies","type":"group","x":-400,"y":200,"operators":[{"type":"repeater","copies":3,"position":[180,0],"startOpacity":0,"endOpacity":1}],
            "layers":[{"id":"child","type":"rect","width":80,"height":50},{"id":"hidden","type":"rect","width":10000,"height":10000,"hidden":true}]}
    ],"compositions":[{"id":"badge","width":800,"height":600,"layers":[{"id":"label","type":"rect","x":200,"y":-60,"width":200,"height":100}]}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    let before=f.project();let history=f.call("history.list",json!({}));
    let check=|cx:&mut gpui::VisualTestContext| cx.update(|_,cx| {
        let st=studio.read(cx);assert!(!st.canvas.fit);
        let vp=viewport.read(cx);let (p,_,Scene::Flat(scene),time)=vp.scene(cx).unwrap() else {panic!()};
        let view=vp.view2(&scene,&p,cx);let bounds=vp.bounds_for_test();let centre=bounds.center();
        let mut lo=[f64::INFINITY;2];let mut hi=[f64::NEG_INFINITY;2];
        for (id,corners) in st.canvas_geometry(&scene,time,&p).unwrap().selection_bounds() {
            if !st.selection.contains(&id) {continue;}
            for p in corners {let q=view.to_screen(p);for i in 0..2 {lo[i]=lo[i].min(q[i]);hi[i]=hi[i].max(q[i]);}}
        }
        for (i,(origin,length,mid)) in [(f32::from(bounds.origin.x),f32::from(bounds.size.width),f32::from(centre.x)),(f32::from(bounds.origin.y),f32::from(bounds.size.height),f32::from(centre.y))].into_iter().enumerate() {
            assert!(((lo[i]+hi[i])/2.-mid as f64).abs()<0.001,"axis {i}: {lo:?} {hi:?}");
            assert!(lo[i]>=origin as f64+31.99 && hi[i]<=origin as f64+length as f64-31.99,"axis {i}: {lo:?} {hi:?}, {bounds:?}");
        }
        assert!(((hi[0]-lo[0])-(f32::from(bounds.size.width) as f64-64.)).abs()<0.01 || ((hi[1]-lo[1])-(f32::from(bounds.size.height) as f64-64.)).abs()<0.01);
    });
    // One command can open Studio and frame a selection before it has a viewport size.
    cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"clipId":clip,"select":["card"],"frame":true,"panel":"none"}),w,cx).unwrap()));cx.run_until_parked();check(cx);
    for width in [1400.,720.,420.] {
        crate::tests::resize(cx,width,900.);cx.run_until_parked();
        for (selection,comp,time) in [(json!(["card"]),"",0.),(json!(["card"]),"",1.),(json!(["child"]),"",1.),(json!(["card","copies"]),"",1.),(json!(["label"]),"badge",0.)] {
            cx.update(|_,cx| {let pb=studio.read(cx).store.read(cx).playback.clone();pb.update(cx,|p,cx|p.seek(time,cx));});cx.run_until_parked();
            cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"select":selection,"composition":comp,"frame":true}),w,cx).unwrap()));cx.run_until_parked();check(cx);
        }
    }
    for selection in [json!([]),json!(["hidden"])] {
        cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"select":selection,"composition":"","frame":true}),w,cx).unwrap()));cx.run_until_parked();
        assert!(cx.update(|_,cx|studio.read(cx).canvas.fit));
    }
    cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"select":["card"],"frame":true}),w,cx).unwrap()));cx.run_until_parked();
    cx.simulate_keystrokes("home");cx.run_until_parked();assert!(cx.update(|_,cx|studio.read(cx).canvas.fit));
    cx.simulate_keystrokes("f");cx.run_until_parked();check(cx);
    cx.simulate_keystrokes("shift-z");cx.run_until_parked();assert!(cx.update(|_,cx|studio.read(cx).canvas.fit));
    // Explicit navigation in the same command is applied after frame, including before layout.
    cx.update(|w,cx|studio.update(cx,|s,cx|s.ui_command(json!({"frame":true,"zoom":2,"pan":[30,40]}),w,cx).unwrap()));cx.run_until_parked();
    assert_eq!(cx.update(|_,cx|studio.read(cx).canvas),super::super::Canvas2d {zoom:2.,pan:[30.,40.],fit:false});
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn viewport_box_cancellation_clears_armed_tools_and_drags_without_edits(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    for (index,scene) in [json!({"layers":[{"id":"item","type":"rect"}]}),json!({"type":"3d","objects":[{"id":"item","type":"box"}]})].into_iter().enumerate() {
        let added=f.call("motion.add",json!({"start":index*10,"duration":4,"scene":scene}));
        let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
        cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.tool=Tool::Select;s.set_selection(vec!["item".into()],cx);}));cx.run_until_parked();
        let before=f.project();let history=f.call("history.list",json!({}));
        let from=cx.update(|_,cx|viewport.read(cx).bounds_for_test().center());let to=from+point(px(40.),px(30.));
        for (dragging,rearm,escape) in [(false,false,false),(true,false,false),(true,true,false),(false,false,true),(true,false,true),(true,true,true)] {
            cx.update(|_,cx|viewport.update(cx,|v,cx|v.arm_box_select(cx)));cx.run_until_parked();
            if dragging {
                cx.simulate_mouse_down(from,MouseButton::Left,gpui::Modifiers::none());cx.run_until_parked();
                cx.simulate_mouse_move(to,Some(MouseButton::Left),gpui::Modifiers::none());cx.run_until_parked();
                if rearm {cx.update(|_,cx|viewport.update(cx,|v,cx|v.arm_box_select(cx)));cx.run_until_parked();}
            }
            if escape {cx.simulate_keystrokes("escape");cx.run_until_parked();}
            else {
                cx.simulate_mouse_down(to,MouseButton::Right,gpui::Modifiers::none());cx.run_until_parked();
                cx.simulate_mouse_up(to,MouseButton::Right,gpui::Modifiers::none());cx.run_until_parked();
            }
            assert!(!cx.update(|_,cx|viewport.read(cx).escapable()),"dragging={dragging}, rearm={rearm}, escape={escape}");
            if dragging {cx.simulate_mouse_up(to,MouseButton::Left,gpui::Modifiers::none());cx.run_until_parked();}
            assert!(!cx.update(|_,cx|viewport.read(cx).escapable()));
            assert!(!cx.update(|_,cx|studio.read(cx).flying));
            assert!(cx.update(|_,cx|studio.read(cx).store.read(cx).menu.is_none()));
            assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),["item"]);
            assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
        }
    }
}

#[gpui::test]
fn box_selection_tracks_visible_instances_of_repeated_groups(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[
        {"id":"group","type":"group","x":-200,"operators":[{"type":"repeater","copies":3,"position":[200,0],"startOpacity":0,"endOpacity":1}],
            "layers":[{"id":"card","type":"rect","width":80,"height":80}]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.tool=Tool::Select;
        s.canvas=super::super::Canvas2d {zoom:1.,pan:[0.;2],fit:false};s.changed(cx);
    }));cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    for (x,expected) in [(-200.,vec![]),(0.,vec!["group","card"]),(200.,vec!["group","card"])] {
        cx.update(|_,cx|viewport.update(cx,|v,cx| {
            let (p,_,Scene::Flat(s),_)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
            v.box_select(view.to_screen([x-20.,-20.]),view.to_screen([x+20.,20.]),SelectionOp::Replace,cx);
        }));
        assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),expected,"at canvas x={x}");
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn layer_box_selection_ignores_invisible_children_in_group_bounds(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[
        {"id":"group","type":"group","layers":[
            {"id":"shown","type":"rect","width":100,"height":100,"x":-200},
            {"id":"hidden","type":"rect","width":100,"height":100,"x":300,"hidden":true},
            {"id":"transparent","type":"rect","width":100,"height":100,"x":100,"opacity":0},
            {"id":"later","type":"rect","width":100,"height":100,"x":200,"start":2}]},
        {"id":"empty","type":"group","layers":[{"id":"emptyHidden","type":"rect","hidden":true}]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.tool=Tool::Select;
        s.canvas=super::super::Canvas2d {zoom:1.,pan:[0.;2],fit:false};s.changed(cx);
    }));cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    for (x,expected) in [(300.,vec![]),(100.,vec![]),(200.,vec![]),(0.,vec![]),(-200.,vec!["group","shown"])] {
        cx.update(|_,cx|viewport.update(cx,|v,cx| {
            let (p,_,Scene::Flat(s),_)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
            v.box_select(view.to_screen([x-20.,-20.]),view.to_screen([x+20.,20.]),SelectionOp::Replace,cx);
        }));
        assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),expected,"at canvas x={x}");
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn layer_box_selection_hits_crossings_and_containment_without_empty_corners(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[{"id":"card","type":"rect","width":400,"height":200,"rotation":30}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.tool=Tool::Select;
        s.canvas=super::super::Canvas2d {zoom:1.,pan:[0.;2],fit:false};s.changed(cx);
    }));cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    let corners=cx.update(|_,cx| {
        let v=viewport.read(cx);let (p,_,Scene::Flat(s),t)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
        studio.read(cx).canvas_geometry(&s,t,&p).unwrap().bounds("card").unwrap().map(|p|view.to_screen(p))
    });
    let (mut lo,mut hi)=([f64::INFINITY;2],[f64::NEG_INFINITY;2]);
    for p in corners {for axis in 0..2 {lo[axis]=lo[axis].min(p[axis]);hi[axis]=hi[axis].max(p[axis]);}}
    let center=[(lo[0]+hi[0])/2.,(lo[1]+hi[1])/2.];
    let inside=[(corners[0][0]+corners[1][0])*0.4+center[0]*0.2,(corners[0][1]+corners[1][1])*0.4+center[1]*0.2];
    for (from,to,hit) in [
        ([inside[0]-3.,inside[1]-3.],[inside[0]+3.,inside[1]+3.],true),
        ([lo[0]-10.,lo[1]+25.],[hi[0]+10.,lo[1]+35.],true),
        ([lo[0]+2.,lo[1]+2.],[lo[0]+8.,lo[1]+8.],false),
    ] {
        cx.update(|_,cx|viewport.update(cx,|v,cx|v.box_select(from,to,SelectionOp::Replace,cx)));
        assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),if hit {vec!["card".to_string()]} else {vec![]},"box {from:?} to {to:?}");
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn viewport_box_selection_respects_visibility_and_helper_controls(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d",
        "camera":{"position":[0,0,4],"target":[0,0,0],"projection":"orthographic","orthoSize":6},
        "cameras":[{"id":"second","position":[0,0,2],"target":[0,0,0]}],
        "lights":[{"id":"light","type":"point","position":[0,0,1]},{"id":"hiddenLight","type":"point","hidden":true}],
        "objects":[{"id":"shown","type":"box"},{"id":"hidden","type":"box","hidden":true},
            {"id":"hiddenParent","type":"group","hidden":true,"children":[{"id":"hiddenChild","type":"box"}]},
            {"id":"later","type":"box","start":2},{"id":"behind","type":"box","position":[0,0,10]}]
    }}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.tool=Tool::Select;
        s.view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho:true,ortho_size:6.,..Default::default()};s.changed(cx);
    }));cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    for (helpers,through,expected) in [(false,false,vec!["shown"]),(true,false,vec!["camera","light","second","shown"]),(true,true,vec!["light","second","shown"])] {
        cx.update(|_,cx|studio.update(cx,|s,cx| {s.helpers=helpers;s.through_camera=through;s.changed(cx);}));cx.run_until_parked();
        cx.update(|_,cx|viewport.update(cx,|v,cx| {
            let b=v.bounds_for_test();let a=[f32::from(b.origin.x) as f64+1.,f32::from(b.origin.y) as f64+1.];
            let z=[a[0]+f32::from(b.size.width) as f64-2.,a[1]+f32::from(b.size.height) as f64-2.];
            v.box_select(a,z,SelectionOp::Replace,cx);
        }));
        let mut actual=cx.update(|_,cx|studio.read(cx).selection.clone());actual.sort();
        assert_eq!(actual,expected,"helpers {helpers}, through camera {through}");
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);

    let added=f.call("motion.add",json!({"start":10,"duration":4,"scene":{"layers":[
        {"id":"shown","type":"rect"},{"id":"hidden","type":"rect","hidden":true},{"id":"transparent","type":"rect","opacity":0},
        {"id":"hiddenParent","type":"group","hidden":true,"layers":[{"id":"hiddenChild","type":"rect"}]},
        {"id":"later","type":"rect","start":2}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    cx.update(|w,cx|studio.update(cx,|s,cx|s.open(clip,w,cx)));cx.run_until_parked();
    let before=f.project();let history=f.call("history.list",json!({}));
    cx.update(|_,cx|viewport.update(cx,|v,cx| {
        let b=v.bounds_for_test();let a=[f32::from(b.origin.x) as f64+1.,f32::from(b.origin.y) as f64+1.];
        let z=[a[0]+f32::from(b.size.width) as f64-2.,a[1]+f32::from(b.size.height) as f64-2.];
        v.box_select(a,z,SelectionOp::Replace,cx);
    }));
    assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),vec!["shown".to_string()]);
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn box_selection_adds_subtracts_and_cancels_for_meshes_and_layers(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"panels","type":"mesh","vertices":[[-3,-1,0],[-1,-1,0],[-1,1,0],[-3,1,0],[1,-1,0],[3,-1,0],[3,1,0],[1,1,0]],"faces":[[0,1,2,3],[4,5,6,7]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {
        s.open(clip,w,cx);s.set_selection(vec!["panels".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.tool=Tool::Select;
        s.view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho:true,ortho_size:6.,..Default::default()};s.through_camera=false;s.changed(cx);
    }));
    cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    let (from,to,faces)=cx.update(|_,cx| {
        let (_,faces,view)=viewport.read(cx).mesh(cx).unwrap();let p=view.project([-3.3,1.3,0.]).unwrap();let q=view.project([-0.7,-1.3,0.]).unwrap();
        (point(px(p[0] as f32),px(p[1] as f32)),point(px(q[0] as f32),px(q[1] as f32)),faces)
    });
    let drag_box=|cx:&mut gpui::VisualTestContext,from,to,modifiers:gpui::Modifiers,cancel| {
        cx.update(|_,cx|viewport.update(cx,|v,cx|v.arm_box_select(cx)));cx.run_until_parked();
        cx.simulate_mouse_down(from,MouseButton::Left,modifiers);cx.simulate_mouse_move(to,Some(MouseButton::Left),modifiers);
        if cancel {cx.simulate_keystrokes("escape");}
        cx.simulate_mouse_up(to,MouseButton::Left,modifiers);cx.run_until_parked();
    };
    let subtract=gpui::Modifiers {shift:true,control:true,..Default::default()};let add=gpui::Modifiers {shift:true,..Default::default()};
    for mode in [SelectMode::Vertex,SelectMode::Edge,SelectMode::Face] {
        cx.update(|_,cx|studio.update(cx,|s,cx| {s.select_mode=mode;s.select_mesh("all",cx).unwrap();}));cx.run_until_parked();
        drag_box(cx,from,to,subtract,false);
        assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.all_vertices(&faces)),vec![4,5,6,7]);
        drag_box(cx,from,to,add,false);
        assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.all_vertices(&faces)),(0..8).collect::<Vec<_>>());
        drag_box(cx,from,to,gpui::Modifiers::none(),true);
        assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.all_vertices(&faces)),(0..8).collect::<Vec<_>>(),"Escape leaves the selection intact");
        drag_box(cx,from,to,gpui::Modifiers::none(),false);
        assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.all_vertices(&faces)),vec![0,1,2,3]);
    }
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);

    let added=f.call("motion.add",json!({"start":10,"duration":4,"scene":{"layers":[
        {"id":"left","type":"rect","width":100,"height":100,"x":-200},{"id":"right","type":"rect","width":100,"height":100,"x":200}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["left".into(),"right".into()],cx);s.tool=Tool::Select;
        s.canvas=super::super::Canvas2d {zoom:1.,pan:[0.;2],fit:false};s.changed(cx);
    }));cx.run_until_parked();let before=f.project();let history=f.call("history.list",json!({}));
    let (from,to)=cx.update(|_,cx| {
        let v=viewport.read(cx);let (p,_,Scene::Flat(s),t)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
        let corners=studio.read(cx).canvas_geometry(&s,t,&p).unwrap().bounds("left").unwrap();let a=view.to_screen(corners[0]);let b=view.to_screen(corners[2]);
        (point(px(a[0] as f32-5.),px(a[1] as f32-5.)),point(px(b[0] as f32+5.),px(b[1] as f32+5.)))
    });
    drag_box(cx,from,to,subtract,false);
    assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),vec!["right".to_string()]);
    drag_box(cx,from,to,add,false);
    assert_eq!(cx.update(|_,cx|studio.read(cx).selection.clone()),vec!["right".to_string(),"left".to_string()]);
    assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
}

#[test]
fn edge_picking_keeps_visible_parts_across_the_camera_plane() {
    let view=View3 {cam:ViewCamera {position:[0.;3],target:[0.,0.,-1.],ortho:true,ortho_size:6.,..Default::default()},x:0.,y:0.,w:600.,h:600.};
    let vertices=[[-1.,0.,-1.],[1.,0.,1.],[0.,2.,-1.]];
    let faces=vec![vec![0,1,2]];
    assert_eq!(nearest_edge(&vertices,&faces,&view,view.project([-0.5,0.,0.]).unwrap()),Some((0,1)));
    assert_eq!(nearest_edge(&vertices,&faces,&view,view.project([0.5,0.,0.]).unwrap()),None);
    let collapsed=[[0.,0.,-1.],[0.,0.,1.],[2.,2.,-1.]];
    assert_eq!(nearest_edge(&collapsed,&faces,&view,[300.,300.]),Some((0,1)),"an edge aligned with the viewing direction retains its visible portion");
    let perspective=View3 {cam:ViewCamera {ortho:false,..view.cam},..view};
    for vertices in [[[-1.,0.,-3.],[2.,0.,1.],[0.,2.,-3.]],[[2.,0.,1.],[-1.,0.,-3.],[0.,2.,-3.]]] {
        assert_eq!(nearest_edge(&vertices,&faces,&perspective,[300.,300.]),Some((0,1)),"a visible edge remains pickable with either endpoint behind the perspective camera");
    }
    assert_eq!(nearest_edge(&[[0.,0.,1.],[0.,0.,2.],[2.,2.,1.]],&faces,&view,[300.,300.]),None,"entirely hidden edges remain unpickable");
}

#[gpui::test]
fn overlapping_components_choose_the_frontmost_depth(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    for (index,ortho) in [true,false].into_iter().enumerate() {
        let back=if ortho {1.} else {9./7.};
        let added=f.call("motion.add",json!({"start":index*10,"duration":4,"scene":{"type":"3d","objects":[
            {"id":"panels","type":"mesh","vertices":[[-1,-1,9],[-back,-back,-1],[back,-back,-1],[back,back,-1],[-back,back,-1],
                [-1,-1,1],[1,-1,1],[1,1,1],[-1,1,1]],"faces":[[1,2,3,4],[5,6,7,8]]}
        ]}}));
        let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
        cx.update(|w,cx|studio.update(cx,|s,cx| {
            s.open(clip,w,cx);s.set_selection(vec!["panels".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
            s.tool=Tool::Select;s.through_camera=false;
            s.view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho,ortho_size:6.,..Default::default()};s.changed(cx);
        }));
        cx.run_until_parked();let history=f.call("history.list",json!({}));
        for (mode,position) in [(SelectMode::Vertex,[-1.,-1.,1.]),(SelectMode::Edge,[0.,-1.,1.])] {
            cx.update(|_,cx|studio.update(cx,|s,cx| {s.select_mode=mode;s.edit_sel=Default::default();s.changed(cx);}));
            cx.run_until_parked();
            let point=cx.update(|_,cx| {
                let (_,_,view)=viewport.read(cx).mesh(cx).unwrap();let p=view.project(position).unwrap();
                gpui::point(px(p[0] as f32),px(p[1] as f32))
            });
            cx.simulate_click(point,gpui::Modifiers::none());cx.run_until_parked();
            let selection=cx.update(|_,cx|studio.read(cx).edit_sel.clone());
            if mode==SelectMode::Vertex {assert_eq!(selection.vertices,vec![5],"ortho {ortho}");}
            else {assert_eq!(selection.edges,vec![(5,6)],"ortho {ortho}");}
        }
        assert_eq!(f.call("history.list",json!({})),history);
    }
}

#[gpui::test]
fn concave_face_picking_keeps_notches_open(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    for (index,(size,mirror)) in [(1.,1.),(1e-8,-1.)].into_iter().enumerate() {
        let vertices:Vec<_>=[[-2.,-2.,0.],[2.,-2.,0.],[2.,-1.,0.],[-1.,-1.,0.],[-1.,1.,0.],[2.,1.,0.],[2.,2.,0.],[-2.,2.,0.],
            [-3.,-3.,-1.],[3.,-3.,-1.],[3.,3.,-1.],[-3.,3.,-1.]].map(|p|math::scale(p,size)).to_vec();
        let added=f.call("motion.add",json!({"start":index*10,"duration":4,"scene":{"type":"3d","objects":[
            {"id":"notched","type":"mesh","scale":[mirror,1,1],"vertices":vertices,"faces":[[0,1,2,3,4,5,6,7],[8,9,10,11]]}
        ]}}));
        let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
        cx.update(|w,cx|studio.update(cx,|s,cx| {
            s.open(clip,w,cx);s.set_selection(vec!["notched".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
            s.select_mode=SelectMode::Face;s.tool=Tool::Select;s.through_camera=false;
            s.view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho:true,ortho_size:8.*size,..Default::default()};s.changed(cx);
        }));
        cx.run_until_parked();let history=f.call("history.list",json!({}));
        for (p,face) in [([0.5,0.,0.],1),([-1.5,0.,0.],0),([1.5,1.5,0.],0)] {
            let point=cx.update(|_,cx| {
                let (_,_,view)=viewport.read(cx).mesh(cx).unwrap();let q=view.project([p[0]*size*mirror,p[1]*size,p[2]*size]).unwrap();
                gpui::point(px(q[0] as f32),px(q[1] as f32))
            });
            cx.simulate_click(point,gpui::Modifiers::none());cx.run_until_parked();
            assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.faces.clone()),vec![face],"scale {size}, mirror {mirror}, point {p:?}");
        }
        assert_eq!(f.call("history.list",json!({})),history,"picking only changes selection");
    }
}

#[gpui::test]
fn tiny_geometry_can_be_picked_and_layer_handles_keep_small_scales(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","camera":{"position":[0,0,8],"target":[0,0,0],"projection":"orthographic","orthoSize":6e-8},"objects":[
        {"id":"mesh","type":"mesh","vertices":[[-1e-8,-1e-8,0],[1e-8,-1e-8,0],[1e-8,1e-8,0],[-1e-8,1e-8,0]],"faces":[[0,1,2,3]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
        s.select_mode=SelectMode::Face;s.tool=Tool::Select;s.view=ViewCamera {position:[0.,0.,8.],target:[0.;3],ortho:true,ortho_size:6e-8,..Default::default()};
        s.through_camera=false;s.changed(cx);
    }));
    cx.run_until_parked();let history=f.call("history.list",json!({}));
    let center=cx.update(|_,cx|viewport.read(cx).bounds_for_test().center());
    cx.simulate_click(center,gpui::Modifiers::none());cx.run_until_parked();
    assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.faces.clone()),vec![0]);
    cx.update(|_,cx|studio.update(cx,|s,cx|s.frame_selection(false,cx)));
    let size=cx.update(|_,cx|studio.read(cx).view.ortho_size);
    assert!(size>2e-8 && size<4e-8,"framing must fit the small geometry: {size}");
    cx.update(|_,cx|studio.update(cx,|s,cx| {
        s.view=ViewCamera::default();s.through_camera=true;s.frame_selection(false,cx);
        assert!(s.view.ortho,"framing from the film camera keeps its projection");
        assert_eq!(s.view.ortho_size,size);
        assert_eq!([s.view.position[0],s.view.position[1]],[0.,0.],"framing keeps the film camera's direction");
        assert!(!s.through_camera);
    }));
    assert_eq!(f.call("history.list",json!({})),history,"picking a small face is only a selection change");

    let added=f.call("motion.add",json!({"start":10,"duration":4,"scene":{"layers":[
        {"id":"card","type":"rect","width":4e10,"height":2e10,"scale":1e-8}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["card".into()],cx);s.tool=Tool::Select;
        s.canvas=super::super::Canvas2d {zoom:1.,pan:[0.;2],fit:false};s.changed(cx);
    }));
    cx.run_until_parked();let before=f.project().clip(clip).unwrap().clone();let history=f.call("history.list",json!({}));
    let (corner,doubled)=cx.update(|_,cx| {
        let v=viewport.read(cx);let (p,_,Scene::Flat(s),t)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
        let (_,handles,anchor)=v.handles2(&studio.read(cx).canvas_geometry(&s,t,&p).unwrap(),&view,cx).unwrap();let corner=handles.iter().find(|(kind,_)|*kind==LayerOp::Scale(1,1)).unwrap().1;
        let point_at=|p:[f64;2]|point(px(p[0] as f32),px(p[1] as f32));
        (point_at(corner),point_at([corner[0]*2.-anchor[0],corner[1]*2.-anchor[1]]))
    });
    cx.simulate_mouse_down(corner,MouseButton::Left,gpui::Modifiers::none());
    assert!(cx.update(|_,cx|matches!(&viewport.read(cx).drag,Some(Drag::Layer(d)) if d.op==LayerOp::Scale(1,1))));
    cx.simulate_mouse_move(doubled,Some(MouseButton::Left),gpui::Modifiers::none());
    cx.simulate_mouse_up(doubled,MouseButton::Left,gpui::Modifiers::none());
    let scaled=|p:&Project| {
        let kimchi_core::ClipContent::Motion {scene:Scene::Flat(s),..}=&p.clip(clip).unwrap().content else {panic!()};
        (s.layers[0].scale/1e-8-2.).abs()<1e-9
    };
    assert!(scaled(&f.settle(cx,scaled)),"mouse scaling must not round a small layer down to zero");
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    f.call("history.undo",json!({}));assert_eq!(f.project().clip(clip),Some(&before));
}

#[gpui::test]
fn mesh_extrusion_previews_keep_tiny_distances_and_cancel_without_drift(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let y=0.123456789123;
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","vertices":[[0,y,0],[1,y,0],[1,y,1],[0,y,1]],"faces":[[3,2,1,0]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];}));
    let before=f.project().clip(clip).unwrap().clone();let history=f.call("history.list",json!({}));
    cx.update(|_,cx|viewport.update(cx,|v,cx|v.start_modal_now(ModalKind::Extrude,cx)));
    wait_mesh(cx,&viewport,|v|v.mesh_modal.as_ref().is_some_and(|m|!m.busy));
    let shape=|p:&Project| {
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&p.clip(clip).unwrap().content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,..}=&s.objects[0].shape else {panic!()};vertices.clone()
    };
    let flat=shape(&f.project());assert_eq!(flat.len(),8);assert!(flat.iter().all(|p|p[1]==y));
    for amount in [0.876543210877,1e-12,-2e-12,0.5,1e-12] {
        cx.update(|_,cx|viewport.update(cx,|v,cx| {v.mesh_modal.as_mut().unwrap().typed=amount.to_string();v.mesh_modal_to(v.mouse,cx);}));
        wait_mesh(cx,&viewport,|v|v.mesh_modal.as_ref().is_some_and(|m|!m.busy));
        let vertices=shape(&f.project());assert_eq!(&vertices[..4],&flat[..4]);
        for (p,origin) in vertices[4..].iter().zip(&flat[4..]) {assert_eq!(*p,[origin[0],y+amount,origin[2]],"each preview uses its original positions");}
    }
    cx.simulate_keystrokes("escape");wait_mesh(cx,&viewport,|v|!v.busy());
    assert_eq!(shape(&f.project()),flat,"cancelling a pull restores its exact starting positions");
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    f.call("history.undo",json!({}));assert_eq!(f.project().clip(clip),Some(&before));
}

#[gpui::test]
fn layer_mouse_drags_restore_animation_on_escape_and_hold_invalid_input(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"layers":[
        {"id":"card","type":"rect","width":400,"height":300,"x":20,"rotation":15,"scale":1.2,
            "keyframes":{"x":[[0,10],[2,80]],"rotation":[[0,10],[2,60]],"scale":[[0,1.2],[2,2]]}}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["card".into()],cx);}));
    let before=f.project().clip(clip).unwrap().clone();
    let at=|op:LayerOp,cx:&mut gpui::VisualTestContext| {
        cx.update(|_,cx|studio.update(cx,|s,cx|s.set_tool(if op==LayerOp::Anchor {Tool::Anchor} else {Tool::Select},cx)));
        cx.run_until_parked();
        cx.update(|_,cx| {
            let v=viewport.read(cx);let (p,_,Scene::Flat(s),t)=v.scene(cx).unwrap() else {panic!()};let view=v.view2(&s,&p,cx);
            let pos=if matches!(op,LayerOp::Move|LayerOp::Anchor) {
                let world=studio.read(cx).canvas_geometry(&s,t,&p).unwrap().transform("card").unwrap();view.to_screen(math::aff_apply(&world,[60.,30.]))
            } else {
                let (corners,handles,anchor)=v.handles2(&studio.read(cx).canvas_geometry(&s,t,&p).unwrap(),&view,cx).unwrap();
                if op==LayerOp::Rotate {
                    let c=corners[0];let d=[c[0]-anchor[0],c[1]-anchor[1]];let n=d[0].hypot(d[1]);
                    [c[0]+d[0]/n*15.,c[1]+d[1]/n*15.]
                } else {handles.iter().find(|(kind,_)|*kind==op).unwrap().1}
            };
            point(px(pos[0] as f32),px(pos[1] as f32))
        })
    };
    for op in [LayerOp::Move,LayerOp::Rotate,LayerOp::Scale(1,1),LayerOp::Anchor] {
        let start=at(op,cx);let moved=start+point(px(40.),px(25.));
        cx.simulate_mouse_down(start,MouseButton::Left,gpui::Modifiers::none());
        assert!(cx.update(|_,cx|matches!(&viewport.read(cx).drag,Some(Drag::Layer(d)) if d.op==op)),"picked {op:?}");
        cx.simulate_mouse_move(moved,Some(MouseButton::Left),gpui::Modifiers::none());
        let changed=|p:&Project|p.clip(clip)!=Some(&before);assert!(changed(&f.settle(cx,changed)),"{op:?} preview");
        cx.simulate_keystrokes("escape");cx.simulate_mouse_up(moved,MouseButton::Left,gpui::Modifiers::none());
        let restored=|p:&Project|p.clip(clip)==Some(&before);assert!(restored(&f.settle(cx,restored)),"{op:?} cancellation");
        assert!(cx.update(|_,cx|studio.read(cx).is_open() && !viewport.read(cx).busy()));
    }
    let start=at(LayerOp::Scale(1,1),cx);let history=f.call("history.list",json!({}));
    cx.simulate_mouse_down(start,MouseButton::Left,gpui::Modifiers::none());cx.simulate_keystrokes("-");
    cx.simulate_mouse_up(start,MouseButton::Left,gpui::Modifiers::none());cx.simulate_keystrokes("enter");
    assert!(cx.update(|_,cx|viewport.read(cx).modal_key.is_some()),"incomplete input keeps the handle's transform active");
    assert_eq!(f.project().clip(clip),Some(&before));
    cx.simulate_keystrokes("backspace 2 enter");
    let doubled=|p:&Project| {
        let kimchi_core::ClipContent::Motion {scene:Scene::Flat(s),..}=&p.clip(clip).unwrap().content else {panic!()};
        (s.layers[0].at(0.).scale-2.4).abs()<1e-12
    };
    assert!(doubled(&f.settle(cx,doubled)));
    assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
    f.call("history.undo",json!({}));assert_eq!(f.project().clip(clip),Some(&before));
}

#[gpui::test]
fn active_component_pivots_follow_pick_order_after_project_refresh(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","vertices":[[-3,-1,0],[-1,-1,0],[-1,1,0],[-3,1,0],[1,-1,0],[3,-1,0],[3,1,0],[1,1,0]],
            "faces":[[0,1,2,3],[4,5,6,7]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
        s.select_mode=SelectMode::Face;s.edit_sel.faces=vec![1,0];s.pivot=gizmo::Pivot::Active;s.changed(cx);
    }));
    f.call("motion.updateLayer",json!({"clipId":clip,"id":"mesh","props":{"castShadow":false}}));
    store_settles(cx,|s|s.project.as_deref()==Some(&f.project()));cx.run_until_parked();
    assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.faces.clone()),vec![1,0],"refresh must not change the active face");
    let before=f.project().clip(clip).unwrap().clone();
    cx.simulate_keystrokes("r");
    let pivot=cx.update(|_,cx|viewport.read(cx).session.as_ref().unwrap().frame.pivot);
    assert_eq!(pivot,[-2.,0.,0.]);
    cx.simulate_keystrokes("z 9 0 enter");
    let matches=|p:&Project| {
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&p.clip(clip).unwrap().content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,..}=&s.objects[0].shape else {panic!()};
        math::len(math::sub(vertices[0],[-1.,-1.,0.]))<1e-12 && math::len(math::sub(vertices[4],[-1.,3.,0.]))<1e-12
    };
    assert!(matches(&f.settle(cx,matches)));
    f.call("history.undo",json!({}));store_settles(cx,|s|s.clip(clip)==Some(&before));
    assert_eq!(cx.update(|_,cx|studio.read(cx).edit_sel.faces.clone()),vec![1,0]);
    assert_eq!(f.project().clip(clip),Some(&before));
}

#[gpui::test]
fn tiny_mesh_transforms_keep_local_axes_and_world_extrusion_distances(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","rotation":[0,0,90],"scale":[1e-8,1e-8,1e-8],
            "vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
        s.select_mode=SelectMode::Vertex;s.edit_sel.vertices=vec![0,1];}));
    let before=f.project().clip(clip).unwrap().clone();
    cx.run_until_parked();cx.simulate_keystrokes("g");
    assert!(cx.update(|_,cx|viewport.read(cx).mesh_target.is_some()),"a small scale is invertible");
    cx.simulate_keystrokes("x x 1 e - 8 enter");
    let shape=|p:&Project| {
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&p.clip(clip).unwrap().content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,..}=&s.objects[0].shape else {panic!()};vertices.clone()
    };
    let moved=|p:&Project|math::len(math::sub(shape(p)[0],[1.,0.,0.]))<1e-12;
    let project=f.settle(cx,moved);assert!(moved(&project));
    assert!(math::len(math::sub(shape(&project)[1],[2.,0.,0.]))<1e-12);
    assert_eq!(&shape(&project)[2..],&[[1.,0.,1.],[0.,0.,1.]]);
    f.call("history.undo",json!({}));store_settles(cx,|s|s.clip(clip)==Some(&before));
    cx.update(|_,cx| {
        studio.update(cx,|s,_| {s.select_mode=SelectMode::Face;s.edit_sel=super::super::EditSel {faces:vec![0],..Default::default()};});
        viewport.update(cx,|v,cx| {v.start_modal_now(ModalKind::Extrude,cx);
            let m=v.mesh_modal.as_mut().expect("tiny face has a valid normal");m.typed="1e-8".into();
            v.mesh_modal_to(v.mouse,cx);v.confirm_modal(cx);
        });
    });
    wait_mesh(cx,&viewport,|v|!v.busy());
    let extruded=shape(&f.project());assert_eq!(extruded.len(),8);
    for p in &extruded[4..] {assert!((p[1]-1.).abs()<1e-12,"extrusion follows the face, not the camera: {p:?}");}
    f.call("history.undo",json!({}));assert_eq!(f.project().clip(clip),Some(&before));
}

#[gpui::test]
fn mesh_gizmos_drag_components_and_escape_restores_the_original_geometry(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","camera":{"position":[0,0,8],"target":[0,0,0]},"objects":[
        {"id":"mesh","type":"mesh","vertices":[[-1,-1,0],[1,-1,0],[1,1,0],[-1,1,0]],"faces":[[0,1,2,3]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
        s.edit_sel.vertices=vec![0,1,2,3];s.tool=Tool::Move;s.through_camera=true;s.changed(cx);}));
    cx.run_until_parked();let before=f.project().clip(clip).unwrap().clone();
    let axis=cx.update(|_,cx| {
        let v=viewport.read(cx);let (p,_,scene,t)=v.scene(cx).unwrap();let Scene::Space(s)=&scene else {panic!()};
        let frame=v.gizmo_frame(s,&scene,t,cx).unwrap();let view=v.view3(s,t,&p,cx);
        let parts=gizmo::parts(&frame,Kind::Grab,&view);let line=parts.iter().find(|p|p.handle==Handle::Axis(0)&&!p.fill).unwrap();
        let a=line.points[0];let b=line.points[1];point(px((a[0]*0.25+b[0]*0.75) as f32),px((a[1]*0.25+b[1]*0.75) as f32))
    });
    let moved=axis+point(px(45.),px(0.));
    cx.simulate_mouse_down(axis,MouseButton::Left,gpui::Modifiers::none());
    assert!(cx.update(|_,cx|viewport.read(cx).gizmo_key.is_some()));
    assert!(cx.update(|_,cx|viewport.read(cx).mesh_target.is_some()));
    cx.simulate_mouse_move(moved,Some(MouseButton::Left),gpui::Modifiers::none());
    let changed=|p:&Project|p.clip(clip)!=Some(&before);assert!(changed(&f.settle(cx,changed)));
    cx.simulate_keystrokes("escape");cx.simulate_mouse_up(moved,MouseButton::Left,gpui::Modifiers::none());
    let restored=|p:&Project|p.clip(clip)==Some(&before);assert!(restored(&f.settle(cx,restored)));
    assert!(!cx.update(|_,cx|viewport.read(cx).busy()));
    // Releasing the handle cannot accept incomplete numeric input.
    cx.simulate_mouse_down(axis,MouseButton::Left,gpui::Modifiers::none());cx.simulate_keystrokes("-");
    cx.simulate_mouse_up(axis,MouseButton::Left,gpui::Modifiers::none());
    assert!(cx.update(|_,cx|viewport.read(cx).modal_key.is_some()));
    cx.simulate_keystrokes("enter");assert!(cx.update(|_,cx|viewport.read(cx).busy()));
    cx.simulate_keystrokes("escape");assert!(restored(&f.settle(cx,restored)));
}

#[gpui::test]
fn mesh_components_move_rotate_and_scale_in_world_or_local_axes_with_exact_undo(cx:&mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","position":[10,0,0],"rotation":[0,0,90],"scale":[2,1,1],
            "vertices":[[0,0,0],[2,0,0],[2,2,0],[0,2,0],[7,8,9]],"faces":[[0,1,2,3]],
            "keyframes":{"position.x":[[0,10],[2,12]]}}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s|s.clip(clip).is_some());
    let studio=cx.update(|_,cx|workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx|studio.read(cx).viewport.clone());
    cx.update(|w,cx|studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();
        s.select_mode=SelectMode::Vertex;s.edit_sel.vertices=vec![0,1];}));
    let before=f.project().clip(clip).unwrap().clone();let history=f.call("history.list",json!({}));
    let shape=|p:&Project| {
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&p.clip(clip).unwrap().content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,..}=&s.objects[0].shape else {panic!()};vertices.clone()
    };
    for (action,entry,expected) in [("g","x 1 enter",[[0.,-1.,0.],[2.,-1.,0.]]),
        ("r","z 9 0 enter",[[1.,-2.,0.],[1.,2.,0.]]),("s","y 2 enter",[[-1.,0.,0.],[3.,0.,0.]]),
        ("g","x x 0 . 2 5 enter",[[0.125,0.,0.],[2.125,0.,0.]])] {
        cx.run_until_parked();cx.simulate_keystrokes(action);wait_mesh(cx,&viewport,|v|v.session.is_some());
        assert!(cx.update(|_,cx|viewport.read(cx).mesh_target.is_some()));
        cx.simulate_keystrokes(entry);
        let matches=|p:&Project|shape(p)[..2].iter().zip(expected).all(|(a,b)|math::len(math::sub(*a,b))<1e-9);
        let p=f.settle(cx,matches);assert!(matches(&p),"{action} {entry}: {:?}",shape(&p));
        let mut restored=p.clip(clip).unwrap().clone();
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&mut restored.content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,..}=&mut s.objects[0].shape else {panic!()};
        vertices[0]=[0.,0.,0.];vertices[1]=[2.,0.,0.];
        assert_eq!(restored,before,"object transforms, animation and unselected vertices stay intact");
        assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
        f.call("history.undo",json!({}));store_settles(cx,|s|s.clip(clip)==Some(&before));assert_eq!(f.project().clip(clip),Some(&before));
    }
    // A queued cancellation must still run when the Studio closes immediately afterwards.
    cx.update(|_,cx|viewport.update(cx,|v,cx| {
        v.start_modal_now(ModalKind::Grab,cx);v.session.as_mut().unwrap().typed="2".into();v.apply_session(cx);
    }));
    cx.update(|_,cx|studio.update(cx,|s,cx|s.close(cx)));
    let start=std::time::Instant::now();
    while cx.update(|_,cx|studio.read(cx).is_open() || studio.read(cx).sender.busy) && start.elapsed()<Duration::from_secs(5) {cx.run_until_parked();std::thread::sleep(Duration::from_millis(10));}
    assert!(!cx.update(|_,cx|studio.read(cx).is_open() || studio.read(cx).sender.busy));
    assert_eq!(f.project().clip(clip),Some(&before),"closing restores selected vertices exactly after pending previews");
}

#[gpui::test]
fn mesh_tools_finish_the_latest_amount_before_accepting_another_gesture(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let before=f.project().clip(clip).unwrap().clone();
    let history=f.call("history.list",json!({}));
    for (cancel,settle_extrusion) in [(false,false),(true,false),(true,true)] {
        cx.update(|w,cx| {
            studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];});
            viewport.update(cx,|v,cx| v.start_modal_now(ModalKind::Extrude,cx));
        });
        if settle_extrusion {
            wait_mesh(cx,&viewport,|v| v.mesh_modal.as_ref().is_some_and(|m| !m.busy));
        }
        cx.update(|_,cx| {
            viewport.update(cx,|v,cx| {
                v.mesh_modal.as_mut().unwrap().typed="2.5".into();
                v.mesh_modal_to(v.mouse,cx);
                v.confirm_modal(cx);
                v.mesh_modal_to([1000.,1000.],cx);
                assert_eq!(v.mesh_modal.as_ref().unwrap().amount,2.5,"confirmation fixes the final amount");
                if cancel {v.cancel_modal(cx);}
                assert!(v.busy(),"finishing waits for pending geometry commands");
                let key=v.mesh_modal.as_ref().unwrap().key.clone();
                v.start_modal_now(ModalKind::Extrude,cx);
                assert_eq!(v.mesh_modal.as_ref().unwrap().key,key,"another gesture cannot steal the completion");
            });
        });
        wait_mesh(cx,&viewport,|v| !v.busy());
        let project=f.project();
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&project.clip(clip).unwrap().content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,faces,..}=&s.objects[0].shape else {panic!()};
        assert_eq!(faces.len(),5);
        assert_eq!(vertices.len(),8);
        for v in &vertices[4..] {assert_eq!(v[1],if cancel {0.} else {2.5});}
        assert_eq!(f.call("history.list",json!({}))["undo"].as_array().unwrap().len(),history["undo"].as_array().unwrap().len()+1);
        f.call("history.undo",json!({}));
        store_settles(cx,|s| s.clip(clip)==Some(&before));
        assert_eq!(f.project().clip(clip),Some(&before));
    }
}

fn wait_mesh(cx:&mut gpui::VisualTestContext,viewport:&Entity<Viewport>,done:impl Fn(&Viewport)->bool) {
    let start=std::time::Instant::now();
    while !cx.update(|_,cx| done(viewport.read(cx))) && start.elapsed()<Duration::from_secs(5) {
        cx.run_until_parked();std::thread::sleep(Duration::from_millis(10));
    }
    assert!(cx.update(|_,cx| done(viewport.read(cx))),"mesh command did not settle: {:?}",cx.update(|_,cx| viewport.read(cx).mesh_modal.clone()));
}

#[gpui::test]
fn mesh_extrusion_follows_displayed_normals_under_parent_rotation_and_scale(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"rig","type":"group","rotation":[0,0,35],"scale":[2,1,3],"children":[
            {"id":"mesh","type":"mesh","position":[1,2,3],"rotation":[90,0,0],"scale":[1,0.5,2],
                "vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]}
        ]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let before=f.project().clip(clip).unwrap().clone();
    let kimchi_core::ClipContent::Motion {scene:Scene::Space(scene),..}=&before.content else {panic!()};
    let (original,_)=model::edit_mesh(scene,&model::worlds(scene,0.),"mesh").unwrap();
    let normal=math::norm(math::cross(math::sub(original[2],original[3]),math::sub(original[1],original[3])));
    cx.update(|w,cx| {
        studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];});
        viewport.update(cx,|v,cx| {
            v.start_modal_now(ModalKind::Extrude,cx);v.mesh_modal.as_mut().unwrap().typed="2".into();
            v.mesh_modal_to(v.mouse,cx);v.confirm_modal(cx);
        });
    });
    wait_mesh(cx,&viewport,|v| !v.busy());
    let project=f.project();
    let kimchi_core::ClipContent::Motion {scene:Scene::Space(scene),..}=&project.clip(clip).unwrap().content else {panic!()};
    let (vertices,faces)=model::edit_mesh(scene,&model::worlds(scene,0.),"mesh").unwrap();
    assert_eq!(faces.len(),5);assert_eq!(vertices.len(),8);
    for p in &vertices[4..] {
        assert!(original.iter().any(|o| math::len(math::sub(*p,math::add(*o,math::scale(normal,2.))))<1e-8),"the pull follows the displayed normal by two world units: {p:?}");
    }
    f.call("history.undo",json!({}));assert_eq!(f.project().clip(clip),Some(&before));
    f.call("motion.updateLayer",json!({"clipId":clip,"id":"mesh","props":{"scale":[0,1,1]}}));
    store_settles(cx,|s| s.project.as_deref()==Some(&f.project()));
    let history=f.call("history.list",json!({}));
    cx.update(|_,cx| viewport.update(cx,|v,cx| {v.start_modal_now(ModalKind::Extrude,cx);assert!(!v.busy());}));
    assert_eq!(f.call("history.list",json!({})),history,"a collapsed transform is rejected before creating geometry");
}

#[gpui::test]
fn mesh_tools_keep_their_original_target_when_the_sidebar_selection_changes(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let objects:Vec<_>=["first","second"].into_iter().map(|id|json!({"id":id,"type":"mesh",
        "vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]})).collect();
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":objects}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let before=f.project().clip(clip).unwrap().clone();
    cx.update(|w,cx| {
        studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["first".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];});
        viewport.update(cx,|v,cx| {
            v.start_modal_now(ModalKind::Extrude,cx);
            v.mesh_modal.as_mut().unwrap().typed="3".into();
            v.mesh_modal_to(v.mouse,cx);
            v.confirm_modal(cx);
        });
        studio.update(cx,|s,cx| {s.set_selection(vec!["second".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.vertices=vec![2];});
    });
    wait_mesh(cx,&viewport,|v| !v.busy());
    let project=f.project();
    let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&project.clip(clip).unwrap().content else {panic!()};
    let kimchi_core::ClipContent::Motion {scene:Scene::Space(original),..}=&before.content else {panic!()};
    assert_eq!(s.objects[1],original.objects[1]);
    let kimchi_core::motion::Shape3d::Mesh {vertices,faces,..}=&s.objects[0].shape else {panic!()};
    assert_eq!(faces.len(),5);
    for v in &vertices[4..] {assert_eq!(v[1],3.);}
    assert_eq!(cx.update(|_,cx| studio.read(cx).edit_sel.clone()),super::super::EditSel {vertices:vec![2],..Default::default()});
    f.call("history.undo",json!({}));
    assert_eq!(f.project().clip(clip),Some(&before));
}

#[gpui::test]
fn closing_studio_waits_for_mesh_cancellation_and_does_not_close_a_reopened_scene(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let before=f.project().clip(clip).unwrap().clone();
    for reopen in [false,true] {
        cx.update(|w,cx| {
            studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];});
            viewport.update(cx,|v,cx| v.start_modal_now(ModalKind::Extrude,cx));
        });
        cx.update(|w,cx| {
            viewport.update(cx,|v,cx| {v.mesh_modal.as_mut().unwrap().typed="2".into();v.mesh_modal_to(v.mouse,cx);});
            studio.update(cx,|s,cx| {
                s.close(cx);s.close(cx);assert!(s.is_open(),"the scene stays available until the cancellation completes");
                if reopen {s.open(clip,w,cx);}
            });
            // Reopening cancels the pending close, but the user can still cancel the tool.
            if reopen {viewport.update(cx,|v,cx| v.cancel_modal(cx));}
        });
        wait_mesh(cx,&viewport,|v| !v.busy());cx.run_until_parked();
        assert_eq!(cx.update(|_,cx| studio.read(cx).is_open()),reopen);
        let project=f.project();
        let kimchi_core::ClipContent::Motion {scene:Scene::Space(scene),..}=&project.clip(clip).unwrap().content else {panic!()};
        let kimchi_core::motion::Shape3d::Mesh {vertices,faces,..}=&scene.objects[0].shape else {panic!()};
        assert_eq!(faces.len(),5);assert!(vertices.iter().all(|v|v[1]==0.),"closing restores the original mesh positions");
        f.call("history.undo",json!({}));store_settles(cx,|s|s.clip(clip)==Some(&before));
        assert_eq!(f.project().clip(clip),Some(&before));
    }
}

#[gpui::test]
fn failed_mesh_pulls_hold_the_last_successful_amount_until_corrected(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[
        {"id":"mesh","type":"mesh","vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]}
    ]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    cx.update(|w,cx| studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];}));
    cx.update(|_,cx| viewport.update(cx,|v,cx| {
        v.start_modal_now(ModalKind::Extrude,cx);
        v.mesh_modal.as_mut().unwrap().typed="1".into();v.mesh_modal_to(v.mouse,cx);
    }));
    wait_mesh(cx,&viewport,|v| v.mesh_modal.as_ref().is_some_and(|m| !m.busy));
    let before=f.project();let history=f.call("history.list",json!({}));
    let vertex_count=cx.update(|_,cx| viewport.update(cx,|v,cx| {
        let m=v.mesh_modal.as_mut().unwrap();let origin=m.origin.as_mut().unwrap();let vertex_count=origin.vertex_count;
        // A stale topology snapshot, as when another client changes a mesh during a gesture.
        origin.vertex_count+=1;m.typed="2".into();
        v.mesh_modal_to(v.mouse,cx);v.confirm_modal(cx);vertex_count
    }));
    wait_mesh(cx,&viewport,|v| v.mesh_modal.as_ref().is_some_and(|m| !m.busy));
    cx.update(|_,cx| viewport.update(cx,|v,cx| {
        let m=v.mesh_modal.as_mut().unwrap();
        assert_eq!(m.applied,1.);assert!(!m.valid);assert!(m.finish.is_none());
        assert_eq!(f.project(),before);assert_eq!(f.call("history.list",json!({})),history);
        m.origin.as_mut().unwrap().vertex_count=vertex_count;m.typed="3".into();v.mesh_modal_to(v.mouse,cx);v.confirm_modal(cx);
    }));
    wait_mesh(cx,&viewport,|v| !v.busy());
    let project=f.project();
    let kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=&project.clip(clip).unwrap().content else {panic!()};
    let kimchi_core::motion::Shape3d::Mesh {vertices,..}=&s.objects[0].shape else {panic!()};
    for v in &vertices[4..] {assert_eq!(v[1],3.,"a failed pull is not counted as applied");}
    // Cancellation must release the tool even if another topology prevents restoration.
    cx.update(|_,cx|viewport.update(cx,|v,cx| {
        v.start_modal_now(ModalKind::Extrude,cx);v.mesh_modal.as_mut().unwrap().typed="1".into();v.mesh_modal_to(v.mouse,cx);
    }));
    wait_mesh(cx,&viewport,|v|v.mesh_modal.as_ref().is_some_and(|m|!m.busy && m.applied==1.));
    let last=f.project();let history=f.call("history.list",json!({}));
    cx.update(|_,cx|viewport.update(cx,|v,cx| {
        v.mesh_modal.as_mut().unwrap().origin.as_mut().unwrap().vertex_count+=1;v.cancel_modal(cx);
    }));
    wait_mesh(cx,&viewport,|v|!v.busy());
    assert_eq!(f.project(),last,"a rejected cancellation never overwrites the changed mesh");
    assert_eq!(f.call("history.list",json!({})),history);
}

#[gpui::test]
fn mesh_completions_from_a_previous_scene_cannot_replace_the_new_selection(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);let mut clips=vec![];
    for start in [0,5] {
        let added=f.call("motion.add",json!({"start":start,"duration":4,"scene":{"type":"3d","objects":[
            {"id":"mesh","type":"mesh","vertices":[[0,0,0],[1,0,0],[1,0,1],[0,0,1]],"faces":[[3,2,1,0]]}
        ]}}));
        clips.push(added["clips"][0]["id"].as_str().unwrap().parse::<Id>().unwrap());
    }
    store_settles(cx,|s| s.clip(clips[1]).is_some());
    let before=f.project().clip(clips[1]).unwrap().clone();
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    cx.update(|w,cx| {
        studio.update(cx,|s,cx| {s.open(clips[0],w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.faces=vec![0];});
        viewport.update(cx,|v,cx| {v.start_modal_now(ModalKind::Extrude,cx);assert!(v.mesh_modal.as_ref().unwrap().busy);});
        // Switch before GPUI can deliver the first command's completion.
        studio.update(cx,|s,cx| {s.open(clips[1],w,cx);s.set_selection(vec!["mesh".into()],cx);s.set_mode(Mode::Edit,cx).unwrap();s.edit_sel.vertices=vec![2];});
    });
    f.settle(cx,|p| match &p.clip(clips[0]).unwrap().content {
        kimchi_core::ClipContent::Motion {scene:Scene::Space(s),..}=>matches!(&s.objects[0].shape,kimchi_core::motion::Shape3d::Mesh {faces,..} if faces.len()==5),_=>false
    });
    cx.run_until_parked();
    let selection=cx.update(|_,cx| studio.read(cx).edit_sel.clone());
    assert_eq!(selection.vertices,vec![2]);assert!(selection.faces.is_empty());
    assert!(!cx.update(|_,cx| viewport.read(cx).busy()));
    assert_eq!(f.project().clip(clips[1]).unwrap(),&before);
}

#[gpui::test]
fn camera_paths_belong_to_the_selected_clip_even_when_camera_ids_match(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let mut clips=vec![];
    for (start,x) in [(0,0),(5,10)] {
        let added=f.call("motion.add",json!({"start":start,"duration":4,"scene":{"type":"3d","camera":{
            "position":[x,0,5],"keyframes":{"position":[[0,[x,0,5]],[4,[x+2,0,5]]]}
        },"objects":[]}}));
        clips.push(added["clips"][0]["id"].as_str().unwrap().parse::<Id>().unwrap());
    }
    store_settles(cx,|s| s.clip(clips[1]).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    for (clip,x) in [(clips[0],0.),(clips[1],10.),(clips[0],0.)] {
        cx.update(|w,cx| studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["camera".into()],cx);}));
        cx.update(|_,cx| viewport.update(cx,|v,cx| {
            v.update_camera_path(cx);
            let path=v.cam_path.as_ref().unwrap();
            assert_eq!(path.clip,clip);
            assert_eq!(path.id,"camera");
            assert_eq!(path.points.first(),Some(&[x,0.,5.]));
            assert_eq!(path.points.last(),Some(&[x+2.,0.,5.]));
            assert_eq!(path.keys,vec![[x,0.,5.],[x+2.,0.,5.]]);
        }));
    }
}

#[gpui::test]
fn viewport_snapping_follows_the_current_control_modifier(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let added=f.call("motion.add",json!({"duration":4,"scene":{"type":"3d","objects":[{"id":"box","type":"box"}]}}));
    let clip:Id=added["clips"][0]["id"].as_str().unwrap().parse().unwrap();
    store_settles(cx,|s| s.clip(clip).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    cx.update(|w,cx| studio.update(cx,|s,cx| {s.open(clip,w,cx);s.set_selection(vec!["box".into()],cx);}));
    cx.run_until_parked();
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let mouse=cx.update(|_,cx| viewport.update(cx,|v,cx| {
        let (p,_,Scene::Space(s),t)=v.scene(cx).unwrap() else {panic!()};
        let view=v.view3(&s,t,&p,cx);
        v.mouse=view.project([0.,0.,0.]).unwrap();
        v.start_modal_now(ModalKind::Grab,cx);
        v.session.as_mut().unwrap().lock(0,false,false,&view);
        let mouse=view.project([1.1,0.,0.]).unwrap();
        point(px(mouse[0] as f32),px(mouse[1] as f32))
    }));
    cx.simulate_mouse_move(mouse,None,gpui::Modifiers {control:true,..Default::default()});
    assert!((cx.update(|_,cx| viewport.read(cx).session.as_ref().unwrap().amount)-1.).abs()<1e-6);
    cx.simulate_mouse_move(mouse+point(px(0.),px(0.01)),None,gpui::Modifiers::none());
    assert!((cx.update(|_,cx| viewport.read(cx).session.as_ref().unwrap().amount)-1.1).abs()<0.001,"releasing Ctrl stops snapping");
    cx.update(|_,cx| studio.update(cx,|s,_| s.snapping=true));
    cx.simulate_mouse_move(mouse,None,gpui::Modifiers::none());
    assert!((cx.update(|_,cx| viewport.read(cx).session.as_ref().unwrap().amount)-1.).abs()<1e-6,"the explicit snapping toggle stays on");
    cx.update(|_,cx| viewport.update(cx,|v,cx| v.cancel_modal(cx)));
}

#[gpui::test]
fn mesh_numeric_input_holds_invalid_amounts_and_requires_correction(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let history=f.call("history.list",json!({}));
    cx.update(|_,cx| viewport.update(cx,|v,cx| {
        v.mesh_modal=Some(MeshModal {kind:ModalKind::Inset,mouse0:[0.,0.],normal:[0.,0.,1.],unit:1.,typed:"-".into(),valid:true,
            amount:0.25,applied:0.,busy:false,key:"gesture:test".into(),clip:Id::new_v4(),id:"mesh".into(),selection:Default::default(),origin:None,finish:None});
        for input in ["-","1e-","1e999","--3"] {
            v.mesh_modal.as_mut().unwrap().typed=input.into();
            v.mesh_modal_to([100.,100.],cx);
            assert_eq!(v.mesh_modal.as_ref().unwrap().amount,0.25);
            assert!(!v.mesh_modal.as_ref().unwrap().valid);
            v.confirm_modal(cx);
            assert!(v.mesh_modal.is_some(),"invalid {input} cannot apply an inset");
        }
        v.mesh_modal.as_mut().unwrap().typed="+2.5e-3".into();
        v.mesh_modal_to([100.,100.],cx);
        assert!(v.mesh_modal.as_ref().unwrap().valid);
        assert_eq!(v.mesh_modal.as_ref().unwrap().amount,0.0025);
        v.cancel_modal(cx);
    }));
    assert_eq!(f.call("history.list",json!({})),history,"previewing and cancelling an inset does not edit the project");
}

#[gpui::test]
fn completed_frames_never_cross_scene_or_composition_boundaries(cx: &mut gpui::TestAppContext) {
    let (f,workspace,cx)=setup(cx);
    let mut clips=vec![];
    for start in [0,5] {
        let added=f.call("motion.add",json!({"start":start,"duration":4,"scene":{
            "layers":[{"id":"box","type":"rect","width":20,"height":20}],
            "compositions":[{"id":"nested","layers":[]}]
        }}));
        clips.push(added["clips"][0]["id"].as_str().unwrap().parse::<Id>().unwrap());
    }
    store_settles(cx,|s| s.clip(clips[1]).is_some());
    let studio=cx.update(|_,cx| workspace.read(cx).editor().read(cx).studio.clone());
    cx.update(|w,cx| studio.update(cx,|s,cx| s.open(clips[0],w,cx)));
    cx.run_until_parked();
    let viewport=cx.update(|_,cx| studio.read(cx).viewport.clone());
    let picture=|| to_image(kimchi_media::tiny_skia::Pixmap::new(2,2).unwrap()).unwrap();
    let old_picture=picture();
    let old_request=cx.update(|_,cx| viewport.update(cx,|v,cx| {
        let req=v.request(cx).unwrap();
        assert!(v.accept_picture(req.clone(),Ok(old_picture.clone()),cx));
        req
    }));
    cx.update(|w,cx| studio.update(cx,|s,cx| s.open(clips[1],w,cx)));
    let new_picture=picture();
    let new_request=cx.update(|_,cx| viewport.update(cx,|v,cx| {
        v.refresh(cx);
        assert!(!v.image.as_ref().is_some_and(|i| Arc::ptr_eq(i,&old_picture)),"switching scenes clears the old picture immediately");
        let req=v.request(cx).unwrap();
        assert!(v.accept_picture(req.clone(),Ok(new_picture.clone()),cx));
        assert!(!v.accept_picture(old_request.clone(),Ok(old_picture.clone()),cx),"a late completion from the old clip is discarded");
        assert!(!v.accept_picture(old_request,Err("old clip failed".into()),cx));
        assert!(v.error.is_none());
        assert!(Arc::ptr_eq(v.image.as_ref().unwrap(),&new_picture));
        req
    }));
    cx.update(|_,cx| studio.update(cx,|s,cx| {s.composition=Some("nested".into());s.changed(cx);}));
    cx.update(|_,cx| viewport.update(cx,|v,cx| {
        v.refresh(cx);
        assert!(!v.image.as_ref().is_some_and(|i| Arc::ptr_eq(i,&new_picture)),"switching compositions also clears the old picture");
        assert!(!v.accept_picture(new_request,Ok(new_picture),cx));
        let req=v.request(cx).unwrap();
        let mut previous_edit=req.clone();
        previous_edit.project=model::SnapshotIdentity::default();
        let image=picture();
        assert!(v.accept_picture(previous_edit,Ok(image.clone()),cx),"an earlier edit of this same scene remains a useful preview");
        let mut other_project=req.clone();other_project.project_id=Id::new_v4();
        assert!(!v.accept_picture(other_project,Ok(picture()),cx));
        let mut other_kind=req;other_kind.three = !other_kind.three;
        assert!(!v.accept_picture(other_kind,Ok(picture()),cx));
        assert!(Arc::ptr_eq(v.image.as_ref().unwrap(),&image));
    }));
}
