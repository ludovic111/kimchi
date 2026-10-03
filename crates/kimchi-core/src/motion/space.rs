//! 3D scenes, like a Blender scene: cameras, lights, a world (environment) and objects —
//! primitives, editable polygon meshes, extruded text and paths, lathed profiles, curves,
//! particles and glTF/OBJ/STL models — each with a material, a modifier stack and constraints.
//! Render settings pick the engine used for final frames (the fast standard renderer or the
//! path tracer) and the camera effects (depth of field, motion blur, bloom).

use super::*;
use crate::motion::particles::ParticleSystem;
use crate::motion::stack::{self, Constraint, Modifier, Pattern};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Scene3d {
    /// Behind everything; none = transparent (what's below shows through).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// The main camera (id `"camera"`).
    #[serde(default)]
    pub camera: Camera,
    /// More cameras, each with an id; `activeCamera` picks the one that films.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cameras: Vec<Camera>,
    /// The camera that films (default the main one). Keyframe it (scene keyframe `activeCamera`)
    /// to cut between cameras.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_camera: Option<String>,
    /// Light that reaches everywhere (0–1); replaced by the environment when there is one.
    #[serde(default = "ambient")]
    pub ambient: f64,
    #[serde(default = "white")]
    pub ambient_color: String,
    /// The world around the scene: lights it from every direction and is reflected by shiny
    /// surfaces (a colour, a gradient, a sky or a panorama picture).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    /// Without lights, a key light and a soft fill are used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<Light>,
    #[serde(default)]
    pub objects: Vec<Object3d>,
    /// Shared materials objects use by id (`"material": "gold"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<Material>,
    /// Soft shadows of objects on the others.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub shadows: bool,
    /// With a background, distant things fade into it (a soft horizon for floors).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub fog: bool,
    #[serde(default, skip_serializing_if = "RenderSettings::is_default")]
    pub render: RenderSettings,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

impl Default for Scene3d {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

pub(super) const SCENE3D_KEYS: &[&str] = &[
    "background", "camera", "cameras", "activeCamera", "ambient", "ambientColor", "environment", "lights", "objects", "materials",
    "shadows", "fog", "render", "keyframes", "animate",
];

/// Scene properties keyframes can animate (`"scene"` item).
pub(super) const SCENE3D_PROPS: &[&str] = &[
    "background", "ambient", "ambientColor", "activeCamera", "environment.strength", "environment.rotation", "environment.color",
    "environment.top", "environment.horizon", "environment.bottom", "render.exposure", "render.bloom", "render.bloomThreshold",
    "render.bloomRadius", "render.motionBlur", "render.ambientOcclusion",
];

impl Scene3d {
    /// The camera with `id` (`"camera"` is the main one).
    pub fn camera_by_id(&self, id: &str) -> Option<&Camera> {
        if id == "camera" || id.is_empty() {
            return Some(&self.camera);
        }
        self.cameras.iter().find(|c| c.id == id)
    }

    /// The id of the camera filming at scene time `t`.
    pub fn active_camera_at(&self, t: f64) -> String {
        let keyed = self.keyframes.get("activeCamera").and_then(|k| value_at(k, t)).and_then(|v| v.as_str().map(str::to_string));
        let id = keyed.or_else(|| self.active_camera.clone()).unwrap_or_else(|| "camera".into());
        if self.camera_by_id(&id).is_some() { id } else { "camera".into() }
    }

    /// The shared material `id`.
    pub fn material(&self, id: &str) -> Option<&Material> {
        self.materials.iter().find(|m| m.id.as_deref() == Some(id))
    }

    /// An object's material with a shared one (`from`) put in place, keeping its own keyframed
    /// changes on top.
    pub fn resolve_material(&self, m: &Material) -> Material {
        match m.from.as_deref().and_then(|id| self.material(id)) {
            Some(shared) => {
                let mut out = shared.clone();
                out.id = None;
                out.from = m.from.clone();
                out
            }
            None => m.clone(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Render settings and the world

/// How final frames are made (the export and "Render" on the timeline); the preview while
/// editing always uses the standard engine, without motion blur.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenderSettings {
    /// `"standard"` (fast: the GPU rasteriser, like Blender's Eevee) or `"path"` (a path tracer on
    /// the CPU, like Cycles: real reflections, refraction, soft shadows and bounced light; slow).
    #[serde(default = "standard")]
    pub engine: String,
    /// Path tracer: rays per pixel.
    #[serde(default = "sixty_four")]
    pub samples: f64,
    /// Path tracer: how many times light bounces.
    #[serde(default = "four")]
    pub bounces: f64,
    /// Path tracer: smooth away the remaining grain.
    #[serde(default = "yes")]
    pub denoise: bool,
    /// Stops brighter (+) or darker (−).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub exposure: f64,
    /// `"standard"` (highlights roll off gently) or `"filmic"` (a wider, film-like range).
    #[serde(default = "standard")]
    pub tone_mapping: String,
    /// Glow around bright things (0 = off).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bloom: f64,
    /// Brightness above which things glow (linear; 1 = white).
    #[serde(default = "point_eight")]
    pub bloom_threshold: f64,
    /// Size of the glow as a share of the frame height.
    #[serde(default = "point_o_five")]
    pub bloom_radius: f64,
    /// Motion blur: the share of a frame the shutter is open (0 = off, 0.5 = a 180° shutter).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub motion_blur: f64,
    /// Moments blended per frame for motion blur.
    #[serde(default = "eight")]
    pub motion_blur_samples: f64,
    /// Darkens creases and contact points (0 = off, 1 = strong); standard engine.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ambient_occlusion: f64,
}

impl Default for RenderSettings {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

impl RenderSettings {
    pub fn is_default(&self) -> bool {
        *self == RenderSettings::default()
    }

    pub fn path_traced(&self) -> bool {
        self.engine == "path"
    }
}

pub(super) const RENDER_KEYS: &[&str] = &[
    "engine", "samples", "bounces", "denoise", "exposure", "toneMapping", "bloom", "bloomThreshold", "bloomRadius", "motionBlur",
    "motionBlurSamples", "ambientOcclusion",
];

/// The world: what lights the scene from all around and what shiny things reflect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    /// `"color"` (one colour), `"gradient"` (top, horizon, bottom), `"sky"` (a daylight sky lit by
    /// the main directional light) or `"image"` (a 360° panorama picture, equirectangular).
    #[serde(rename = "type", default = "gradient_kind")]
    pub kind: String,
    #[serde(default = "env_color")]
    pub color: String,
    #[serde(default = "sky_top")]
    pub top: String,
    #[serde(default = "sky_horizon")]
    pub horizon: String,
    #[serde(default = "sky_bottom")]
    pub bottom: String,
    /// A panorama picture (media item by id, name or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default = "one")]
    pub strength: f64,
    /// Degrees the world turns around the vertical axis.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// Show the world behind the objects (instead of the background colour).
    #[serde(default, skip_serializing_if = "is_false")]
    pub visible: bool,
}

pub(super) const ENVIRONMENT_KEYS: &[&str] = &["type", "color", "top", "horizon", "bottom", "image", "strength", "rotation", "visible"];
pub const ENVIRONMENT_TYPES: &[&str] = &["color", "gradient", "sky", "image"];

// ---------------------------------------------------------------------------------------------
// Cameras

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Camera {
    /// `"camera"` for the main one; extra cameras have their own.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default = "camera_position")]
    pub position: Vec3,
    /// The point it looks at.
    #[serde(default)]
    pub target: Vec3,
    /// Vertical field of view in degrees (perspective).
    #[serde(default = "forty")]
    pub fov: f64,
    /// Degrees around the viewing direction.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub roll: f64,
    /// `"perspective"` or `"orthographic"` (no perspective: parallel lines stay parallel).
    #[serde(default = "perspective", skip_serializing_if = "is_perspective")]
    pub projection: String,
    /// Orthographic: world units the frame's height covers.
    #[serde(default = "five", skip_serializing_if = "is_five")]
    pub ortho_size: f64,
    /// Depth of field: the f-number (smaller = blurrier background, like a real lens); 0 = off.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub f_stop: f64,
    /// Distance in focus; 0 = the target's distance.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub focus_distance: f64,
    /// lookAt, followPath… applied after keyframes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
    #[serde(default, skip_serializing_if = "Expressions::is_empty")]
    pub expressions: Expressions,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

pub(super) const CAMERA_KEYS: &[&str] = &[
    "id", "position", "target", "fov", "roll", "projection", "orthoSize", "fStop", "focusDistance", "constraints", "expressions",
    "keyframes", "animate",
];
pub(super) const CAMERA_PROPS: &[&str] = &[
    "position", "position.x", "position.y", "position.z", "target", "target.x", "target.y", "target.z", "fov", "roll", "orthoSize",
    "fStop", "focusDistance",
];

impl Default for Camera {
    fn default() -> Self {
        serde_json::from_value(serde_json::json!({})).expect("defaults")
    }
}

impl Camera {
    pub fn at(&self, t: f64) -> Camera {
        let mut c = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                let _ = c.set(name, &v);
            }
        }
        c.keyframes.clear();
        c
    }

    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        match name {
            "fov" => self.fov = n()?,
            "roll" => self.roll = n()?,
            "orthoSize" => self.ortho_size = n()?,
            "fStop" => self.f_stop = n()?,
            "focusDistance" => self.focus_distance = n()?,
            "projection" => {
                let p = v.as_str().ok_or("projection is perspective or orthographic")?;
                if !matches!(p, "perspective" | "orthographic") {
                    return Err(format!("projection is perspective or orthographic, not `{p}`"));
                }
                self.projection = p.to_string();
            }
            _ => {
                if self.position.set_named(name, "position", v)?
                    || self.target.set_named(name, "target", v)?
                    || stack::set_in(&mut self.constraints, name, v)?
                {
                    return Ok(());
                }
                return Err(format!("a camera has no property `{name}`; use {}, or constraints.<id>.<param>", CAMERA_PROPS.join(", ")));
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<KeyValue> {
        let n = |v: f64| Some(KeyValue::Number(v));
        match name {
            "fov" => n(self.fov),
            "roll" => n(self.roll),
            "orthoSize" => n(self.ortho_size),
            "fStop" => n(self.f_stop),
            "focusDistance" => n(self.focus_distance),
            "projection" => Some(KeyValue::from(self.projection.as_str())),
            _ => self.position.get_named(name, "position").or_else(|| self.target.get_named(name, "target")).or_else(|| stack::get_in(&self.constraints, name)),
        }
    }

    pub fn orthographic(&self) -> bool {
        self.projection == "orthographic"
    }
}

// ---------------------------------------------------------------------------------------------
// Lights

pub const LIGHT_TYPES: &[&str] = &["directional", "point", "spot", "area"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Light {
    pub id: String,
    /// `"directional"` (the sun: only `direction` matters), `"point"` (a bulb at `position`),
    /// `"spot"` (a cone from `position` along `direction`) or `"area"` (a glowing rectangle at
    /// `position` facing `direction`: soft light and shadows).
    #[serde(rename = "type", default = "directional")]
    pub kind: String,
    #[serde(default = "white")]
    pub color: String,
    #[serde(default = "one")]
    pub intensity: f64,
    #[serde(default = "light_position")]
    pub position: Vec3,
    /// Where it shines towards (directional, spot, area).
    #[serde(default = "light_direction")]
    pub direction: Vec3,
    /// Point and spot lights: distance where the light has faded out (0 = never).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub range: f64,
    /// Spot: the cone's full angle in degrees.
    #[serde(default = "forty_five", skip_serializing_if = "is_forty_five")]
    pub angle: f64,
    /// Spot: how soft the cone's edge is (0 = hard, 1 = all soft).
    #[serde(default = "point_two", skip_serializing_if = "is_point_two")]
    pub blend: f64,
    /// Size of the light (world units): an area light's width and height; the radius of a
    /// point or spot bulb, or the sun's softness (degrees), for softer shadows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[f64; 2]>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cast_shadows: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
    #[serde(default, skip_serializing_if = "Expressions::is_empty")]
    pub expressions: Expressions,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

pub(super) const LIGHT_KEYS: &[&str] = &[
    "id", "type", "color", "intensity", "position", "direction", "range", "angle", "blend", "size", "castShadows", "hidden",
    "constraints", "expressions", "keyframes", "animate",
];
pub(super) const LIGHT_PROPS: &[&str] = &[
    "intensity", "color", "range", "angle", "blend", "size", "position", "position.x", "position.y", "position.z", "direction",
    "direction.x", "direction.y", "direction.z",
];

impl Light {
    pub fn at(&self, t: f64) -> Light {
        let mut l = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                let _ = l.set(name, &v);
            }
        }
        l.keyframes.clear();
        l
    }

    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        match name {
            "intensity" => self.intensity = n()?,
            "range" => self.range = n()?,
            "angle" => self.angle = n()?,
            "blend" => self.blend = n()?,
            "size" => {
                let s = v.as_vec(2).ok_or("size takes [width, height] or one number")?;
                self.size = Some([s[0], s[1]]);
            }
            "color" => {
                let c = v.as_str().ok_or("color takes a colour")?;
                check_color(c, "color")?;
                self.color = c.to_string();
            }
            _ => {
                if self.position.set_named(name, "position", v)?
                    || self.direction.set_named(name, "direction", v)?
                    || stack::set_in(&mut self.constraints, name, v)?
                {
                    return Ok(());
                }
                return Err(format!("a light has no property `{name}`; use {}", LIGHT_PROPS.join(", ")));
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<KeyValue> {
        let n = |v: f64| Some(KeyValue::Number(v));
        match name {
            "intensity" => n(self.intensity),
            "range" => n(self.range),
            "angle" => n(self.angle),
            "blend" => n(self.blend),
            "size" => Some(KeyValue::Vector(self.size.unwrap_or([0.0, 0.0]).to_vec())),
            "color" => Some(KeyValue::from(self.color.as_str())),
            _ => self.position.get_named(name, "position").or_else(|| self.direction.get_named(name, "direction")).or_else(|| stack::get_in(&self.constraints, name)),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Objects

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Object3d {
    pub id: String,
    #[serde(flatten)]
    pub shape: Shape3d,
    #[serde(default, skip_serializing_if = "Vec3::is_zero")]
    pub position: Vec3,
    /// Degrees around x, then y, then z.
    #[serde(default, skip_serializing_if = "Vec3::is_zero")]
    pub rotation: Vec3,
    #[serde(default = "Vec3::one", skip_serializing_if = "Vec3::is_one")]
    pub scale: Vec3,
    /// Its own material, or a shared one by id (`"material": "gold"`).
    #[serde(default)]
    pub material: Material,
    /// Applied to the shape in order (see `motion.guide` 3d: subdivision, mirror, array, bevel…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modifiers: Vec<Modifier>,
    /// Applied to the position and rotation after keyframes (lookAt, followPath…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
    /// Objects that move with this one (their position is relative to it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Object3d>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cast_shadow: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub start: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// Properties computed from a formula every frame (`{"rotation.y": "time * 90"}`).
    #[serde(default, skip_serializing_if = "Expressions::is_empty")]
    pub expressions: Expressions,
    #[serde(default, alias = "animate", skip_serializing_if = "Keyframes::is_empty")]
    pub keyframes: Keyframes,
}

pub(super) const OBJECT_KEYS: &[&str] = &[
    "id", "type", "position", "rotation", "scale", "material", "modifiers", "constraints", "children", "castShadow", "start", "end",
    "hidden", "expressions", "keyframes", "animate",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Shape3d {
    #[serde(rename_all = "camelCase")]
    Box {
        #[serde(default = "Vec3::one")]
        size: Vec3,
        /// Rounded edges (0–0.5 of the smallest side).
        #[serde(default, skip_serializing_if = "is_zero")]
        bevel: f64,
    },
    #[serde(rename_all = "camelCase")]
    Sphere {
        #[serde(default = "half")]
        radius: f64,
        /// Segments around (rings are half as many); 0 = smooth.
        #[serde(default, skip_serializing_if = "is_zero")]
        segments: f64,
    },
    /// A sphere made of triangles (even faces everywhere; detail 0 = an icosahedron).
    #[serde(rename_all = "camelCase")]
    Icosphere {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "two")]
        detail: f64,
    },
    #[serde(rename_all = "camelCase")]
    Cylinder {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "one")]
        height: f64,
        /// Sides; 0 = smooth (3 = a prism, 6 = a hexagon).
        #[serde(default, skip_serializing_if = "is_zero")]
        segments: f64,
    },
    #[serde(rename_all = "camelCase")]
    Cone {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "one")]
        height: f64,
        /// Sides; 0 = smooth (4 = a pyramid).
        #[serde(default, skip_serializing_if = "is_zero")]
        segments: f64,
    },
    #[serde(rename_all = "camelCase")]
    Capsule {
        #[serde(default = "point_three")]
        radius: f64,
        /// Total height, the round ends included.
        #[serde(default = "one")]
        height: f64,
    },
    #[serde(rename_all = "camelCase")]
    Torus {
        #[serde(default = "half")]
        radius: f64,
        #[serde(default = "tube")]
        tube: f64,
    },
    #[serde(rename_all = "camelCase")]
    Plane {
        #[serde(default = "one")]
        width: f64,
        #[serde(default = "one")]
        height: f64,
    },
    /// A plane split into rows × cols faces (to bend with wave, displace…), lying flat (y up).
    #[serde(rename_all = "camelCase")]
    Grid {
        #[serde(default = "two")]
        width: f64,
        #[serde(default = "two")]
        height: f64,
        #[serde(default = "thirty_two")]
        rows: f64,
        #[serde(default = "thirty_two")]
        cols: f64,
    },
    /// Extruded 3D letters, centred on the object's position.
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        #[serde(default = "manrope")]
        font_family: String,
        #[serde(default = "bold")]
        font_weight: f64,
        /// Letter height (em size) in world units.
        #[serde(default = "one")]
        size: f64,
        /// Thickness in world units.
        #[serde(default = "tube")]
        depth: f64,
        #[serde(default)]
        align: crate::TextAlign,
        #[serde(default, skip_serializing_if = "is_zero")]
        letter_spacing: f64,
        /// Rounded front edges, world units.
        #[serde(default, skip_serializing_if = "is_zero")]
        bevel: f64,
    },
    /// A flat outline (SVG path data: a logo, an icon) given thickness, centred, its larger side
    /// `size` world units. Holes (inner outlines) stay holes.
    #[serde(rename_all = "camelCase")]
    Extrude {
        d: String,
        #[serde(default = "two")]
        size: f64,
        #[serde(default = "tube")]
        depth: f64,
        /// Rounded front edges, world units.
        #[serde(default, skip_serializing_if = "is_zero")]
        bevel: f64,
    },
    /// A profile turned around the vertical axis (vases, bottles, glasses): points are
    /// `[distance from the axis, height]`.
    #[serde(rename_all = "camelCase")]
    Lathe {
        profile: Vec<[f64; 2]>,
        #[serde(default = "forty_eight")]
        segments: f64,
        /// Degrees turned (360 = all the way round).
        #[serde(default = "three_sixty")]
        angle: f64,
    },
    /// A line through points (smooth or straight). With a radius it renders as a tube (animate
    /// trimEnd 0 → 1 to draw it on); without, it is an invisible path for followPath.
    #[serde(rename_all = "camelCase")]
    Curve {
        points: Vec<[f64; 3]>,
        #[serde(default, skip_serializing_if = "is_false")]
        closed: bool,
        /// Curved through the points (true) or straight lines between them.
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        smooth: bool,
        #[serde(default, skip_serializing_if = "is_zero")]
        radius: f64,
        #[serde(default = "eight", skip_serializing_if = "is_eight")]
        sides: f64,
        #[serde(default, skip_serializing_if = "is_zero")]
        trim_start: f64,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        trim_end: f64,
    },
    /// A polygon mesh you can edit face by face (`motion.editMesh`): `vertices` are points,
    /// `faces` list vertex indices counter-clockwise seen from outside (3 or more each).
    #[serde(rename_all = "camelCase")]
    Mesh {
        vertices: Vec<[f64; 3]>,
        faces: Vec<Vec<u32>>,
        /// Texture coordinates per face corner (same shape as `faces`); made automatically when absent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        uvs: Vec<Vec<[f64; 2]>>,
        /// Edges bent more than this (degrees) look sharp; smaller bends look smooth.
        #[serde(default = "thirty", skip_serializing_if = "is_thirty")]
        auto_smooth: f64,
    },
    /// Many small things born, moving and dying over time (sparks, snow, confetti, dust).
    Particles(ParticleSystem),
    /// A glTF/GLB, OBJ or STL model file (absolute path, or a media item), centred and scaled to 2 units.
    Model { src: String },
    /// A flat picture (media item by id, name or path) facing the camera's default direction.
    #[serde(rename_all = "camelCase")]
    Image {
        asset: String,
        /// Width in world units; the height follows the picture.
        #[serde(default = "two")]
        width: f64,
    },
    /// Holds children only (an empty, a null).
    Group {},
}

pub(super) const SHAPE_KEYS: &[(&str, &[&str])] = &[
    ("box", &["size", "bevel"]),
    ("sphere", &["radius", "segments"]),
    ("icosphere", &["radius", "detail"]),
    ("cylinder", &["radius", "height", "segments"]),
    ("cone", &["radius", "height", "segments"]),
    ("capsule", &["radius", "height"]),
    ("torus", &["radius", "tube"]),
    ("plane", &["width", "height"]),
    ("grid", &["width", "height", "rows", "cols"]),
    ("text", &["text", "fontFamily", "fontWeight", "size", "depth", "align", "letterSpacing", "bevel"]),
    ("extrude", &["d", "size", "depth", "bevel"]),
    ("lathe", &["profile", "segments", "angle"]),
    ("curve", &["points", "closed", "smooth", "radius", "sides", "trimStart", "trimEnd"]),
    ("mesh", &["vertices", "faces", "uvs", "autoSmooth"]),
    ("particles", crate::motion::particles::PARTICLE_KEYS),
    ("model", &["src"]),
    ("image", &["asset", "width"]),
    ("group", &[]),
];

/// Animatable properties per shape (besides the common ones).
pub(super) const SHAPE_PROPS: &[(&str, &[&str])] = &[
    ("box", &["size", "size.x", "size.y", "size.z", "bevel"]),
    ("sphere", &["radius"]),
    ("icosphere", &["radius"]),
    ("cylinder", &["radius", "height"]),
    ("cone", &["radius", "height"]),
    ("capsule", &["radius", "height"]),
    ("torus", &["radius", "tube"]),
    ("plane", &["width", "height"]),
    ("grid", &["width", "height"]),
    ("text", &["text", "size", "depth", "letterSpacing", "bevel"]),
    ("extrude", &["d", "size", "depth", "bevel"]),
    ("lathe", &["angle"]),
    ("curve", &["radius", "trimStart", "trimEnd", "points"]),
    ("mesh", &[]),
    ("particles", crate::motion::particles::PARTICLE_PROPS),
    ("model", &[]),
    ("image", &["width"]),
    ("group", &[]),
];

pub(super) const OBJECT_PROPS: &[&str] = &[
    "position", "position.x", "position.y", "position.z", "x", "y", "z", "rotation", "rotation.x", "rotation.y", "rotation.z", "scale",
    "scale.x", "scale.y", "scale.z", "color", "opacity", "metallic", "roughness", "emissive", "emissiveIntensity", "transmission", "ior",
    "clearcoat", "textureScale", "pattern.<param>", "modifiers.<id>.<param>", "constraints.<id>.<param>",
];

impl Shape3d {
    pub fn name(&self) -> &'static str {
        match self {
            Shape3d::Box { .. } => "box",
            Shape3d::Sphere { .. } => "sphere",
            Shape3d::Icosphere { .. } => "icosphere",
            Shape3d::Cylinder { .. } => "cylinder",
            Shape3d::Cone { .. } => "cone",
            Shape3d::Capsule { .. } => "capsule",
            Shape3d::Torus { .. } => "torus",
            Shape3d::Plane { .. } => "plane",
            Shape3d::Grid { .. } => "grid",
            Shape3d::Text { .. } => "text",
            Shape3d::Extrude { .. } => "extrude",
            Shape3d::Lathe { .. } => "lathe",
            Shape3d::Curve { .. } => "curve",
            Shape3d::Mesh { .. } => "mesh",
            Shape3d::Particles(_) => "particles",
            Shape3d::Model { .. } => "model",
            Shape3d::Image { .. } => "image",
            Shape3d::Group {} => "group",
        }
    }

    /// Animatable properties of this shape (beyond the ones every object has).
    pub fn props(&self) -> &'static [&'static str] {
        SHAPE_PROPS.iter().find(|(k, _)| *k == self.name()).map_or(&[], |(_, p)| p)
    }

    fn set(&mut self, name: &str, v: &KeyValue) -> Result<bool, String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        let s = || v.as_str().map(str::to_string).ok_or_else(|| format!("{name} takes text"));
        match (self, name) {
            (Shape3d::Box { size, .. }, _) if name == "size" || name.starts_with("size.") => {
                size.set_named(name, "size", v)?;
            }
            (Shape3d::Box { bevel, .. } | Shape3d::Text { bevel, .. } | Shape3d::Extrude { bevel, .. }, "bevel") => *bevel = n()?,
            (
                Shape3d::Sphere { radius, .. }
                | Shape3d::Icosphere { radius, .. }
                | Shape3d::Cylinder { radius, .. }
                | Shape3d::Cone { radius, .. }
                | Shape3d::Capsule { radius, .. }
                | Shape3d::Torus { radius, .. }
                | Shape3d::Curve { radius, .. },
                "radius",
            ) => *radius = n()?,
            (Shape3d::Cylinder { height, .. } | Shape3d::Cone { height, .. } | Shape3d::Capsule { height, .. } | Shape3d::Plane { height, .. } | Shape3d::Grid { height, .. }, "height") => {
                *height = n()?
            }
            (Shape3d::Torus { tube, .. }, "tube") => *tube = n()?,
            (Shape3d::Plane { width, .. } | Shape3d::Image { width, .. } | Shape3d::Grid { width, .. }, "width") => *width = n()?,
            (Shape3d::Text { text, .. }, "text") => *text = s()?,
            (Shape3d::Text { size, .. } | Shape3d::Extrude { size, .. }, "size") => *size = n()?,
            (Shape3d::Text { depth, .. } | Shape3d::Extrude { depth, .. }, "depth") => *depth = n()?,
            (Shape3d::Text { letter_spacing, .. }, "letterSpacing") => *letter_spacing = n()?,
            (Shape3d::Extrude { d, .. }, "d") => {
                let p = s()?;
                crate::path::parse(&p).map_err(|e| format!("d: {e}"))?;
                *d = p;
            }
            (Shape3d::Lathe { angle, .. }, "angle") => *angle = n()?,
            (Shape3d::Curve { trim_start, .. }, "trimStart") => *trim_start = n()?,
            (Shape3d::Curve { trim_end, .. }, "trimEnd") => *trim_end = n()?,
            (Shape3d::Curve { points, .. }, "points") => {
                let flat = match v {
                    KeyValue::Vector(f) if f.len() % 3 == 0 && !f.is_empty() => f.clone(),
                    _ => return Err("points takes the curve's points flattened: [x0, y0, z0, x1, y1, z1, …]".into()),
                };
                *points = flat.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
            }
            (Shape3d::Particles(p), _) => return p.set(name, v),
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn get(&self, name: &str) -> Option<KeyValue> {
        let n = |v: f64| Some(KeyValue::Number(v));
        match (self, name) {
            (Shape3d::Box { size, .. }, _) if size.get_named(name, "size").is_some() => size.get_named(name, "size"),
            (Shape3d::Box { bevel, .. } | Shape3d::Text { bevel, .. } | Shape3d::Extrude { bevel, .. }, "bevel") => n(*bevel),
            (
                Shape3d::Sphere { radius, .. }
                | Shape3d::Icosphere { radius, .. }
                | Shape3d::Cylinder { radius, .. }
                | Shape3d::Cone { radius, .. }
                | Shape3d::Capsule { radius, .. }
                | Shape3d::Torus { radius, .. }
                | Shape3d::Curve { radius, .. },
                "radius",
            ) => n(*radius),
            (Shape3d::Cylinder { height, .. } | Shape3d::Cone { height, .. } | Shape3d::Capsule { height, .. } | Shape3d::Plane { height, .. } | Shape3d::Grid { height, .. }, "height") => {
                n(*height)
            }
            (Shape3d::Torus { tube, .. }, "tube") => n(*tube),
            (Shape3d::Plane { width, .. } | Shape3d::Image { width, .. } | Shape3d::Grid { width, .. }, "width") => n(*width),
            (Shape3d::Text { text, .. }, "text") => Some(KeyValue::from(text.as_str())),
            (Shape3d::Text { size, .. } | Shape3d::Extrude { size, .. }, "size") => n(*size),
            (Shape3d::Text { depth, .. } | Shape3d::Extrude { depth, .. }, "depth") => n(*depth),
            (Shape3d::Text { letter_spacing, .. }, "letterSpacing") => n(*letter_spacing),
            (Shape3d::Extrude { d, .. }, "d") => Some(KeyValue::from(d.as_str())),
            (Shape3d::Lathe { angle, .. }, "angle") => n(*angle),
            (Shape3d::Curve { trim_start, .. }, "trimStart") => n(*trim_start),
            (Shape3d::Curve { trim_end, .. }, "trimEnd") => n(*trim_end),
            (Shape3d::Curve { points, .. }, "points") => Some(KeyValue::Vector(points.iter().flatten().copied().collect())),
            (Shape3d::Particles(p), _) => p.get(name),
            _ => None,
        }
    }

    /// Checks what serde can't (mesh indices, profiles, paths).
    pub(super) fn check(&self, id: &str) -> Result<(), String> {
        match self {
            Shape3d::Mesh { vertices, faces, uvs, .. } => {
                for (i, f) in faces.iter().enumerate() {
                    if f.len() < 3 {
                        return Err(format!("mesh \"{id}\": face {i} has {} vertices; a face needs 3 or more", f.len()));
                    }
                    if let Some(bad) = f.iter().find(|&&v| v as usize >= vertices.len()) {
                        return Err(format!("mesh \"{id}\": face {i} uses vertex {bad}, but there are {} vertices (0–{})", vertices.len(), vertices.len().saturating_sub(1)));
                    }
                }
                if !uvs.is_empty() && (uvs.len() != faces.len() || uvs.iter().zip(faces).any(|(u, f)| u.len() != f.len())) {
                    return Err(format!("mesh \"{id}\": uvs has one [u, v] per face corner, in the same shape as faces"));
                }
            }
            Shape3d::Lathe { profile, .. } if profile.len() < 2 => {
                return Err(format!("lathe \"{id}\": the profile needs at least two [radius, height] points"));
            }
            Shape3d::Curve { points, .. } if points.len() < 2 => return Err(format!("curve \"{id}\": a curve needs at least two points")),
            Shape3d::Extrude { d, .. } => crate::path::parse(d).map(|_| ()).map_err(|e| format!("extrude \"{id}\": path data: {e}"))?,
            Shape3d::Particles(p) => p.check().map_err(|e| format!("particles \"{id}\": {e}"))?,
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Materials

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaterialFields {
    /// Shared materials (in the scene's `materials`) have an id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default = "grey")]
    pub color: String,
    /// 0 = plastic/paint, 1 = metal.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub metallic: f64,
    /// 0 = mirror-smooth, 1 = matte.
    #[serde(default = "half")]
    pub roughness: f64,
    /// Glows with this colour regardless of lights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emissive: Option<String>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub emissive_intensity: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    /// Glass: 0 = opaque, 1 = clear (light passes and bends through it; the path tracer refracts).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub transmission: f64,
    /// How much glass bends light (1.0 air, 1.33 water, 1.5 glass, 2.4 diamond).
    #[serde(default = "glass_ior", skip_serializing_if = "is_glass_ior")]
    pub ior: f64,
    /// A clear varnish over the colour (car paint, lacquer), 0–1.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub clearcoat: f64,
    /// A picture wrapped on the surface (media item by id, name or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// A procedural pattern instead of a picture (checker, marble, wood…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<Pattern>,
    /// How many times the texture or pattern repeats across the surface ([u, v] or one number).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_scale: Option<[f64; 2]>,
    /// Faceted look instead of smooth.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flat: bool,
    /// No lighting: the colour as is.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unlit: bool,
}

/// A surface: colour, how metallic and rough, glow, glass, texture or pattern. In JSON an
/// object (its own material) or a string naming a shared one (`"gold"`); `{"from": "gold"}`
/// is the same.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// A shared material this one is (its own fields then come from it).
    pub from: Option<String>,
    pub fields: MaterialFields,
}

impl std::ops::Deref for Material {
    type Target = MaterialFields;
    fn deref(&self) -> &MaterialFields {
        &self.fields
    }
}

impl std::ops::DerefMut for Material {
    fn deref_mut(&mut self) -> &mut MaterialFields {
        &mut self.fields
    }
}

impl Default for Material {
    fn default() -> Self {
        Material { from: None, fields: serde_json::from_value(serde_json::json!({})).expect("defaults") }
    }
}

impl Serialize for Material {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.from {
            Some(id) => id.serialize(s),
            None => self.fields.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Material {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match v {
            Value::String(id) => Ok(Material { from: Some(id), ..Material::default() }),
            Value::Object(mut o) => {
                let from = match o.remove("from") {
                    Some(Value::String(f)) => Some(f),
                    _ => None,
                };
                let fields = serde_json::from_value(Value::Object(o)).map_err(serde::de::Error::custom)?;
                Ok(Material { from, fields })
            }
            _ => Err(serde::de::Error::custom("a material is an object or the id of a shared material")),
        }
    }
}

pub(super) const MATERIAL_KEYS: &[&str] = &[
    "id", "from", "color", "metallic", "roughness", "emissive", "emissiveIntensity", "opacity", "transmission", "ior", "clearcoat",
    "texture", "pattern", "textureScale", "flat", "unlit",
];

impl Material {
    pub(super) fn set(&mut self, name: &str, v: &KeyValue) -> Result<bool, String> {
        let n = || v.as_f64().ok_or_else(|| format!("{name} takes a number"));
        let c = || -> Result<String, String> {
            let s = v.as_str().ok_or_else(|| format!("{name} takes a colour like \"#ff5a36\""))?;
            check_color(s, name)?;
            Ok(s.to_string())
        };
        match name {
            "color" => self.color = c()?,
            "opacity" => self.opacity = n()?,
            "metallic" => self.metallic = n()?,
            "roughness" => self.roughness = n()?,
            "emissive" => self.emissive = Some(c()?),
            "emissiveIntensity" => self.emissive_intensity = n()?,
            "transmission" => self.transmission = n()?,
            "ior" => self.ior = n()?,
            "clearcoat" => self.clearcoat = n()?,
            "textureScale" => {
                let s = v.as_vec(2).ok_or("textureScale takes [u, v] or one number")?;
                self.texture_scale = Some([s[0], s[1]]);
            }
            _ => {
                let Some(param) = name.strip_prefix("pattern.") else { return Ok(false) };
                let p = self.pattern.as_mut().ok_or("the material has no pattern; give it one first (\"pattern\": {\"type\": \"checker\"})")?;
                p.set(param, v)?;
            }
        }
        Ok(true)
    }

    pub(super) fn get(&self, name: &str) -> Option<KeyValue> {
        let n = |v: f64| Some(KeyValue::Number(v));
        match name {
            "color" => Some(KeyValue::from(self.color.as_str())),
            "opacity" => n(self.opacity),
            "metallic" => n(self.metallic),
            "roughness" => n(self.roughness),
            "emissive" => self.emissive.as_deref().map(KeyValue::from),
            "emissiveIntensity" => n(self.emissive_intensity),
            "transmission" => n(self.transmission),
            "ior" => n(self.ior),
            "clearcoat" => n(self.clearcoat),
            "textureScale" => Some(KeyValue::Vector(self.texture_scale.unwrap_or([1.0, 1.0]).to_vec())),
            _ => self.pattern.as_ref()?.get(name.strip_prefix("pattern.")?),
        }
    }

    pub(super) fn check(&self, what: &str, refs: &mut Vec<String>) -> Result<(), String> {
        check_color(&self.color, &format!("{what} color"))?;
        if let Some(e) = &self.emissive {
            check_color(e, &format!("{what} emissive"))?;
        }
        if let Some(p) = &self.pattern {
            p.check(refs).map_err(|e| format!("{what}: {e}"))?;
        }
        if let Some(f) = &self.from {
            refs.push(format!("material:{f}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Object properties

impl Object3d {
    pub fn visible_at(&self, t: f64) -> bool {
        !self.hidden && t + 1e-9 >= self.start && self.end.is_none_or(|e| t < e - 1e-9)
    }

    /// The object with its keyframes applied at scene time `t` (children included). Expressions
    /// and constraints need the whole scene: see [`Scene::evaluate`].
    pub fn at(&self, t: f64) -> Object3d {
        let mut o = self.clone();
        for (name, keys) in &self.keyframes {
            if let Some(v) = value_at(keys, t) {
                let _ = o.set(name, &v);
            }
        }
        o.keyframes.clear();
        for c in o.children.iter_mut() {
            *c = c.at(t);
        }
        o
    }

    pub fn set(&mut self, name: &str, v: &KeyValue) -> Result<(), String> {
        let alias = match name {
            "x" => "position.x",
            "y" => "position.y",
            "z" => "position.z",
            other => other,
        };
        if self.position.set_named(alias, "position", v)?
            || self.rotation.set_named(alias, "rotation", v)?
            || self.scale.set_named(alias, "scale", v)?
            || self.material.set(alias, v)?
            || self.shape.set(alias, v)?
            || stack::set_in(&mut self.modifiers, alias, v)?
            || stack::set_in(&mut self.constraints, alias, v)?
        {
            return Ok(());
        }
        Err(format!(
            "a {} has no animatable property `{name}`. Every object: {}. {}: {}.",
            self.shape.name(),
            OBJECT_PROPS.join(", "),
            self.shape.name(),
            self.shape.props().join(", ")
        ))
    }

    pub fn get(&self, name: &str) -> Option<KeyValue> {
        let alias = match name {
            "x" => "position.x",
            "y" => "position.y",
            "z" => "position.z",
            other => other,
        };
        self.position
            .get_named(alias, "position")
            .or_else(|| self.rotation.get_named(alias, "rotation"))
            .or_else(|| self.scale.get_named(alias, "scale"))
            .or_else(|| self.material.get(alias))
            .or_else(|| self.shape.get(alias))
            .or_else(|| stack::get_in(&self.modifiers, alias))
            .or_else(|| stack::get_in(&self.constraints, alias))
    }

    /// Every animatable property name it has now (for the window's keyframe lists).
    pub fn prop_names(&self) -> Vec<String> {
        let mut out: Vec<String> = OBJECT_PROPS.iter().filter(|p| !p.contains('<') && !matches!(**p, "x" | "y" | "z")).map(|p| p.to_string()).collect();
        out.extend(self.shape.props().iter().map(|p| p.to_string()));
        if let Some(p) = &self.material.pattern
            && let Some(spec) = p.spec()
        {
            out.extend(spec.params.iter().filter(|q| q.kind.animatable()).map(|q| format!("pattern.{}", q.name)));
        }
        out.extend(stack::animatable_names(&self.modifiers));
        out.extend(stack::animatable_names(&self.constraints));
        out
    }

    pub(super) fn name_items(&mut self) {
        stack::name_items(&mut self.modifiers);
        stack::name_items(&mut self.constraints);
    }
}

// ---------------------------------------------------------------------------------------------
// Vectors

/// Three numbers; in JSON `[x, y, z]`, or one number for all three (`"scale": 2`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3(pub [f64; 3]);

impl Vec3 {
    pub fn one() -> Vec3 {
        Vec3([1.0; 3])
    }

    pub(super) fn is_zero(&self) -> bool {
        self.0 == [0.0; 3]
    }

    pub(super) fn is_one(&self) -> bool {
        self.0 == [1.0; 3]
    }

    /// Sets it when `name` is `base` or `base.x|y|z`; false when `name` is something else.
    pub(super) fn set_named(&mut self, name: &str, base: &str, v: &KeyValue) -> Result<bool, String> {
        if name == base {
            let p = v.as_vec(3).ok_or_else(|| format!("{base} takes [x, y, z] or one number"))?;
            self.0 = [p[0], p[1], p[2]];
            return Ok(true);
        }
        let Some(axis) = name.strip_prefix(base).and_then(|r| r.strip_prefix('.')) else { return Ok(false) };
        let i = match axis {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return Err(format!("{name}: the axes are x, y and z")),
        };
        self.0[i] = v.as_f64().ok_or_else(|| format!("{name} takes a number"))?;
        Ok(true)
    }

    /// `base` as a vector or `base.x|y|z` as a number.
    pub(super) fn get_named(&self, name: &str, base: &str) -> Option<KeyValue> {
        if name == base {
            return Some(KeyValue::Vector(self.0.to_vec()));
        }
        let axis = name.strip_prefix(base)?.strip_prefix('.')?;
        let i = ["x", "y", "z"].iter().position(|a| *a == axis)?;
        Some(KeyValue::Number(self.0[i]))
    }
}

impl Serialize for Vec3 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Vec3 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = KeyValue::deserialize(d)?;
        v.as_vec(3).map(|p| Vec3([p[0], p[1], p[2]])).ok_or_else(|| serde::de::Error::custom("expected [x, y, z] or a number"))
    }
}

// serde defaults only 3D uses
fn standard() -> String {
    "standard".into()
}
fn perspective() -> String {
    "perspective".into()
}
fn is_perspective(s: &str) -> bool {
    s == "perspective"
}
fn is_five(v: &f64) -> bool {
    *v == 5.0
}
fn gradient_kind() -> String {
    "gradient".into()
}
fn env_color() -> String {
    "#808080".into()
}
fn sky_top() -> String {
    "#5b7fb8".into()
}
fn sky_horizon() -> String {
    "#d8e2ee".into()
}
fn sky_bottom() -> String {
    "#3a3632".into()
}
fn sixty_four() -> f64 {
    64.0
}
fn point_eight() -> f64 {
    0.8
}
fn point_o_five() -> f64 {
    0.05
}
fn point_two() -> f64 {
    0.2
}
fn is_point_two(v: &f64) -> bool {
    *v == 0.2
}
fn point_three() -> f64 {
    0.3
}
fn forty_five() -> f64 {
    45.0
}
fn is_forty_five(v: &f64) -> bool {
    *v == 45.0
}
fn thirty() -> f64 {
    30.0
}
fn is_thirty(v: &f64) -> bool {
    *v == 30.0
}
fn thirty_two() -> f64 {
    32.0
}
fn forty_eight() -> f64 {
    48.0
}
fn three_sixty() -> f64 {
    360.0
}
fn glass_ior() -> f64 {
    1.45
}
fn is_glass_ior(v: &f64) -> bool {
    *v == 1.45
}
