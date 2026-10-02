// The 3D shading on the GPU. Mirrors `shade` in space/mod.rs line for line, so a scene looks the
// same whichever renderer draws it.

struct Globals {
    viewproj: mat4x4<f32>,
    shadow_vp: mat4x4<f32>,
    // xyz: camera position; w: number of lights.
    eye: vec4<f32>,
    // rgb: sky ambient; w: index of the shadowing light (-1: none).
    sky: vec4<f32>,
    ground: vec4<f32>,
    // rgb: fog colour (linear); w: 1 when there is fog.
    fog_color: vec4<f32>,
    // x: fog start distance, y: where it is complete.
    fog: vec4<f32>,
    // xyz: direction the light travels (directional) or position (point); w: 1 for point lights.
    light_v: array<vec4<f32>, 8>,
    // rgb: colour × intensity; w: range (point lights, 0 = no fall-off).
    light_c: array<vec4<f32>, 8>,
};

struct Item {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    base: vec4<f32>,
    // rgb: emissive; w: 1 when unlit.
    emissive: vec4<f32>,
    // metallic, roughness.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_cmp: sampler_comparison;

@group(1) @binding(0) var<uniform> it: Item;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) uv: vec2<f32>) -> VsOut {
    let w = it.model * vec4<f32>(p, 1.0);
    var o: VsOut;
    o.pos = g.viewproj * w;
    o.world = w.xyz;
    o.normal = (it.normal * vec4<f32>(n, 0.0)).xyz;
    o.uv = uv;
    return o;
}

fn to_srgb(v: f32) -> f32 {
    let x = clamp(v, 0.0, 1.0);
    if (x <= 0.0031308) {
        return x * 12.92;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn shoulder(v: f32) -> f32 {
    if (v <= 0.8) {
        return max(v, 0.0);
    }
    return 0.8 + 0.2 * (1.0 - exp(-(v - 0.8) / 0.2));
}

fn lit_at(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let li = i32(g.sky.w);
    if (li < 0) {
        return 1.0;
    }
    let l = g.light_v[li].xyz;
    let p = world + n * 0.01 - l * 0.005;
    let q4 = g.shadow_vp * vec4<f32>(p, 1.0);
    let q = q4.xyz / q4.w;
    if (q.z < 0.0 || q.z >= 1.0) {
        return 1.0;
    }
    let size = vec2<f32>(textureDimensions(shadow_map));
    let st = vec2<f32>(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5);
    let base = floor(st * size);
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let c = clamp(base + vec2<f32>(f32(dx), f32(dy)), vec2<f32>(0.0), size - 1.0);
            sum += textureSampleCompareLevel(shadow_map, shadow_cmp, (c + 0.5) / size, q.z - 0.002);
        }
    }
    return sum / 9.0;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let s = textureSample(tex, samp, in.uv);
    let base = it.base * s;
    var rgb: vec3<f32>;
    let a = clamp(base.a, 0.0, 1.0);
    if (it.emissive.w > 0.5) {
        rgb = base.rgb + it.emissive.rgb;
        return vec4<f32>(to_srgb(rgb.r) * a, to_srgb(rgb.g) * a, to_srgb(rgb.b) * a, a);
    }
    let v = normalize(g.eye.xyz - in.world);
    var n = normalize(in.normal);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    let metallic = it.params.x;
    let rough = clamp(it.params.y, 0.04, 1.0);
    let f0 = vec3<f32>(0.04) + (base.rgb - vec3<f32>(0.04)) * metallic;
    let diffuse = base.rgb * (1.0 - metallic);
    let shininess = clamp(2.0 / (rough * rough * rough * rough) - 2.0, 1.0, 4096.0);
    let norm = (shininess + 8.0) / 8.0 * (1.0 - 0.6 * rough);
    var out = vec3<f32>(0.0);
    let count = i32(g.eye.w);
    let main = i32(g.sky.w);
    let lit = lit_at(in.world, n);
    for (var i = 0; i < count; i++) {
        let lv = g.light_v[i];
        let lc = g.light_c[i];
        var dir: vec3<f32>;
        var k = 1.0;
        if (lv.w > 0.5) {
            let d = lv.xyz - in.world;
            let dist = length(d);
            if (lc.w > 0.0) {
                let f = clamp(1.0 - dist / lc.w, 0.0, 1.0);
                k = f * f;
            }
            dir = normalize(d);
        } else {
            dir = -lv.xyz;
        }
        let ndl = max(dot(n, dir), 0.0);
        if (ndl <= 0.0 || k <= 0.0) {
            continue;
        }
        let h = normalize(dir + v);
        let ndh = max(dot(n, h), 0.0);
        let vdh = max(dot(v, h), 0.0);
        let fr = pow(1.0 - vdh, 5.0);
        let spec = norm * pow(ndh, shininess);
        var sh = 1.0;
        if (i == main) {
            sh = lit;
        }
        let fc = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * fr;
        out += (diffuse * (vec3<f32>(1.0) - fc) + fc * spec) * lc.rgb * ndl * k * sh;
    }
    let up = n.y * 0.5 + 0.5;
    let ndv = max(dot(n, v), 0.0);
    let r = n * (2.0 * dot(n, v)) - v;
    let rup = r.y * 0.5 + 0.5;
    let gloss = (1.0 - rough) * (1.0 - rough);
    let amb = g.ground.rgb + (g.sky.rgb - g.ground.rgb) * up;
    let env = (g.ground.rgb + (g.sky.rgb - g.ground.rgb) * rup) * 2.0;
    let fre = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * pow(1.0 - ndv, 5.0) * gloss;
    out += diffuse * amb + env * fre * gloss + it.emissive.rgb;
    if (g.fog_color.w > 0.5) {
        var k = clamp((length(g.eye.xyz - in.world) - g.fog.x) / max(g.fog.y - g.fog.x, 0.001), 0.0, 1.0);
        k = k * k * (3.0 - 2.0 * k);
        out = out + (g.fog_color.rgb - out) * k;
    }
    return vec4<f32>(to_srgb(shoulder(out.r)) * a, to_srgb(shoulder(out.g)) * a, to_srgb(shoulder(out.b)) * a, a);
}

// Shadow pass: depth from the light.
struct ShadowGlobals {
    viewproj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> sg: ShadowGlobals;
@group(1) @binding(0) var<uniform> sit: Item;

@vertex
fn vs_shadow(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    return sg.viewproj * sit.model * vec4<f32>(p, 1.0);
}
