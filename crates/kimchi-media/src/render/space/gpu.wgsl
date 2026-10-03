// The 3D shading on the GPU. Mirrors `shade` in space/mod.rs (and the world in space/env.rs)
// line for line, so a scene looks the same whichever renderer draws it.

struct Light {
    // xyz: direction the light travels (sun) or its position; w: 0 sun, 1 point, 2 spot, 3 area.
    v: vec4<f32>,
    // rgb: colour × intensity; w: range (point and spot; 0 = no fall-off).
    color: vec4<f32>,
    // xyz: where spot and area lights face; w: its shadow map (-1: none).
    dir: vec4<f32>,
    // cos_outer, cos_inner, size (area: width, height; bulb radius; sun softness in radians).
    cone: vec4<f32>,
};

struct Shadow {
    vp: mat4x4<f32>,
    // x: 1 = orthographic; y: soft edge (world units); z, w: near and far (perspective).
    p: vec4<f32>,
    // x: world size of the map (orthographic) or tan of half its field of view.
    q: vec4<f32>,
};

struct Globals {
    viewproj: mat4x4<f32>,
    // xyz: camera position; w: number of lights.
    eye: vec4<f32>,
    // rgb: sky ambient; w: 1 when the camera is orthographic.
    sky: vec4<f32>,
    // rgb: ground ambient; w: exposure (a multiplier).
    ground: vec4<f32>,
    // xyz: the camera's viewing direction; w: 1 = linear output (camera effects follow).
    forward: vec4<f32>,
    // xyz: the camera's right; w: tan of half the vertical field of view.
    right: vec4<f32>,
    // xyz: the camera's up; w: orthographic height.
    up: vec4<f32>,
    // x, y: output size in pixels; z: 1 = filmic tone mapping.
    screen: vec4<f32>,
    // rgb: fog colour (linear); w: 1 when there is fog.
    fog_color: vec4<f32>,
    // x: fog start distance, y: where it is complete.
    fog: vec4<f32>,
    // x: world kind (-1 none, 0 colour, 1 gradient, 2 sky, 3 image); y: strength; z: rotation
    // (radians); w: 1 = drawn behind the objects.
    env: vec4<f32>,
    env_color: vec4<f32>,
    env_top: vec4<f32>,
    env_horizon: vec4<f32>,
    env_bottom: vec4<f32>,
    // xyz: towards the sun.
    sun: vec4<f32>,
    sun_color: vec4<f32>,
    // Irradiance, spherical harmonics (already / π).
    sh: array<vec4<f32>, 9>,
    lights: array<Light, 8>,
    shadows: array<Shadow, 4>,
};

struct Item {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    base: vec4<f32>,
    // rgb: emissive; w: 1 when unlit.
    emissive: vec4<f32>,
    // metallic, roughness, transmission, ior.
    params: vec4<f32>,
    // clearcoat, bump strength, 1 when there is a bump map.
    extra: vec4<f32>,
    // xy: texture repeats.
    uv: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_maps: texture_depth_2d_array;
@group(0) @binding(2) var env_maps: texture_2d<f32>;
@group(0) @binding(3) var env_image: texture_2d<f32>;
@group(0) @binding(4) var env_samp: sampler;

@group(1) @binding(0) var<uniform> it: Item;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
@group(1) @binding(3) var bump_tex: texture_2d<f32>;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;
const ENV_LEVELS: f32 = 6.0;

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

fn tone(v: f32) -> f32 {
    if (g.screen.z > 0.5) {
        let x = max(v, 0.0);
        return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
    }
    return shoulder(v);
}

fn untone(v: f32) -> f32 {
    let y = clamp(v, 0.0, 0.9999);
    if (g.screen.z > 0.5) {
        let a = 2.51 - 2.43 * y;
        let b = 0.03 - 0.59 * y;
        let c = -0.14 * y;
        if (abs(a) < 1e-6) {
            return -c / b;
        }
        return max((-b + sqrt(max(b * b - 4.0 * a * c, 0.0))) / (2.0 * a), 0.0);
    }
    if (y <= 0.8) {
        return y;
    }
    return 0.8 - 0.2 * log(1.0 - (y - 0.8) / 0.2);
}

// A shaded colour as written: tone mapped and sRGB-encoded, or linear for the camera effects
// (unlit colours pre-distorted so they come out as given), premultiplied.
fn finish(c: vec3<f32>, alpha: f32, unlit: bool) -> vec4<f32> {
    let a = clamp(alpha, 0.0, 1.0);
    let k = g.ground.w;
    var o: vec3<f32>;
    if (g.forward.w > 0.5) {
        if (unlit) {
            o = vec3<f32>(untone(c.r), untone(c.g), untone(c.b)) / k;
        } else {
            o = c;
        }
    } else if (unlit) {
        o = vec3<f32>(to_srgb(c.r), to_srgb(c.g), to_srgb(c.b));
    } else {
        o = vec3<f32>(to_srgb(tone(c.r * k)), to_srgb(tone(c.g * k)), to_srgb(tone(c.b * k)));
    }
    return vec4<f32>(o * a, a);
}

fn smooth01(x: f32) -> f32 {
    let t = clamp(x, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn smoothstep_(a: f32, b: f32, x: f32) -> f32 {
    return smooth01((x - a) / (b - a));
}

// ---- The world (env.rs) ----

fn rotate_y(d: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec3<f32>(d.x * c + d.z * s, d.y, -d.x * s + d.z * c);
}

fn dir_to_uv(d: vec3<f32>) -> vec2<f32> {
    let u = atan2(d.x, -d.z) / TAU + 0.5;
    let v = acos(clamp(d.y, -1.0, 1.0)) / PI;
    return vec2<f32>(u, v);
}

fn gradient(y: f32) -> vec3<f32> {
    if (y >= 0.0) {
        return mix(g.env_horizon.rgb, g.env_top.rgb, sqrt(min(y, 1.0)));
    }
    return mix(g.env_horizon.rgb, g.env_bottom.rgb, sqrt(min(-y, 1.0)));
}

fn sky(d: vec3<f32>) -> vec3<f32> {
    let sun = g.sun.xyz;
    let e = sun.y;
    let day = smoothstep_(-0.1, 0.35, e);
    let zenith = mix(vec3<f32>(0.05, 0.05, 0.12), vec3<f32>(0.12, 0.26, 0.62), day);
    let horizon = mix(vec3<f32>(0.85, 0.45, 0.25), vec3<f32>(0.68, 0.78, 0.92), smoothstep_(-0.05, 0.4, e));
    let light = 0.08 + 0.92 * smoothstep_(-0.12, 0.2, e);
    let y = d.y;
    var c: vec3<f32>;
    if (y >= 0.0) {
        c = mix(horizon, zenith, pow(min(y, 1.0), 0.45));
    } else {
        let ground = vec3<f32>(0.16, 0.15, 0.14) * light;
        c = mix(horizon * 0.5, ground, sqrt(min(-y, 1.0)));
    }
    let cs = max(dot(d, sun), 0.0);
    let haze = 0.35 * pow(cs, 8.0) + 1.5 * pow(cs, 64.0);
    let above = smoothstep_(-0.15, 0.05, y);
    return (c * light + g.sun_color.rgb * haze * above) * g.env.y;
}

// What the world shows in direction `d` (linear).
fn env_radiance(dir: vec3<f32>) -> vec3<f32> {
    let d = rotate_y(dir, -g.env.z);
    let kind = i32(g.env.x + 0.5);
    if (kind == 0) {
        return g.env_color.rgb;
    }
    if (kind == 1) {
        return gradient(d.y);
    }
    if (kind == 2) {
        return sky(d);
    }
    return textureSampleLevel(env_image, env_samp, dir_to_uv(d), 0.0).rgb * g.env.y;
}

fn env_reflect(dir: vec3<f32>, rough: f32) -> vec3<f32> {
    let uv = dir_to_uv(rotate_y(dir, -g.env.z));
    return textureSampleLevel(env_maps, env_samp, uv, clamp(rough, 0.0, 1.0) * (ENV_LEVELS - 1.0)).rgb;
}

fn env_irradiance(dir: vec3<f32>) -> vec3<f32> {
    let n = rotate_y(dir, -g.env.z);
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var c = g.sh[0].rgb * 0.282095;
    c += g.sh[1].rgb * (0.488603 * y);
    c += g.sh[2].rgb * (0.488603 * z);
    c += g.sh[3].rgb * (0.488603 * x);
    c += g.sh[4].rgb * (1.092548 * x * y);
    c += g.sh[5].rgb * (1.092548 * y * z);
    c += g.sh[6].rgb * (0.315392 * (3.0 * z * z - 1.0));
    c += g.sh[7].rgb * (1.092548 * x * z);
    c += g.sh[8].rgb * (0.546274 * (x * x - y * y));
    return max(c, vec3<f32>(0.0));
}

// The environment's blurred reflection, or the sky/ground ambient without one.
fn world_light(d: vec3<f32>, rough: f32) -> vec3<f32> {
    if (g.env.x > -0.5) {
        return env_reflect(d, rough);
    }
    let up = d.y * 0.5 + 0.5;
    return (g.ground.rgb + (g.sky.rgb - g.ground.rgb) * up) * 2.0;
}

// ---- Shadows ----

fn shadow_distance(s: Shadow, z: f32) -> f32 {
    let r = s.p.w / (s.p.z - s.p.w);
    return r * s.p.z / (z + r);
}

fn shadow_lit(li: i32, world: vec3<f32>, n: vec3<f32>) -> f32 {
    let l = g.lights[li];
    let si = i32(l.dir.w);
    if (si < 0) {
        return 1.0;
    }
    let s = g.shadows[si];
    var to_light: vec3<f32>;
    if (l.v.w > 0.5) {
        to_light = normalize(l.v.xyz - world);
    } else {
        to_light = -l.v.xyz;
    }
    let p = world + n * 0.01 + to_light * 0.005;
    let q4 = s.vp * vec4<f32>(p, 1.0);
    if (q4.w <= 1e-6) {
        return 1.0;
    }
    let q = q4.xyz / q4.w;
    let ortho = s.p.x > 0.5;
    if (q.z < 0.0 || q.z >= 1.0 || (!ortho && (abs(q.x) > 1.0 || abs(q.y) > 1.0))) {
        return 1.0;
    }
    let size = f32(textureDimensions(shadow_maps).x);
    let sx = (q.x * 0.5 + 0.5) * size;
    let sy = (0.5 - q.y * 0.5) * size;
    var texel: f32;
    if (ortho) {
        texel = s.q.x / size;
    } else {
        texel = 2.0 * shadow_distance(s, q.z) * s.q.x / size;
    }
    let radius = clamp(s.p.y / max(texel, 1e-6), 0.0, 12.0);
    var taps = 1;
    var step = 1.0;
    if (radius > 1.0) {
        taps = 2;
        step = radius / 2.0;
    }
    var mine: f32;
    if (ortho) {
        mine = q.z - 0.002;
    } else {
        mine = shadow_distance(s, q.z) * 0.995;
    }
    let last = i32(size) - 1;
    var sum = 0.0;
    for (var dy = -taps; dy <= taps; dy++) {
        for (var dx = -taps; dx <= taps; dx++) {
            let x = clamp(i32(floor(sx + f32(dx) * step)), 0, last);
            let y = clamp(i32(floor(sy + f32(dy) * step)), 0, last);
            var d = textureLoad(shadow_maps, vec2<i32>(x, y), si, 0);
            if (!ortho) {
                d = shadow_distance(s, d);
            }
            if (mine <= d) {
                sum += 1.0;
            }
        }
    }
    return sum / f32((2 * taps + 1) * (2 * taps + 1));
}

fn lobe_shininess(rough: f32) -> f32 {
    return clamp(2.0 / (rough * rough * rough * rough) - 2.0, 1.0, 4096.0);
}

fn lobe_norm(rough: f32, shininess: f32) -> f32 {
    return (shininess + 8.0) / 8.0 * (1.0 - 0.6 * rough);
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    // Everything that needs neighbouring pixels first (uniform control flow).
    let uvs = in.uv * it.uv.xy;
    let s = textureSample(tex, samp, uvs);
    let dp1 = dpdx(in.world);
    let dp2 = dpdy(in.world);
    let duv1 = dpdx(uvs);
    let duv2 = dpdy(uvs);

    let base = it.base * s;
    if (it.emissive.w > 0.5) {
        return finish(base.rgb + it.emissive.rgb, base.a, true);
    }
    let p = in.world;
    var v: vec3<f32>;
    if (g.sky.w > 0.5) {
        v = -g.forward.xyz;
    } else {
        v = normalize(g.eye.xyz - p);
    }
    var n = normalize(in.normal);
    if (dot(n, v) < 0.0) {
        n = -n;
    }
    if (it.extra.z > 0.5) {
        // The surface gradient of the height map (the CPU works it out per triangle; these are
        // the same within a triangle).
        let det = duv1.x * duv2.y - duv1.y * duv2.x;
        if (abs(det) > 1e-12) {
            let dpdu = (dp1 * duv2.y - dp2 * duv1.y) / det;
            let dpdv = (dp2 * duv1.x - dp1 * duv2.x) / det;
            let dims = vec2<f32>(textureDimensions(bump_tex));
            let du = 1.0 / dims.x;
            let dv = 1.0 / dims.y;
            let hu = (textureSampleLevel(bump_tex, samp, uvs + vec2<f32>(du, 0.0), 0.0).r - textureSampleLevel(bump_tex, samp, uvs - vec2<f32>(du, 0.0), 0.0).r) / (2.0 * du);
            let hv = (textureSampleLevel(bump_tex, samp, uvs + vec2<f32>(0.0, dv), 0.0).r - textureSampleLevel(bump_tex, samp, uvs - vec2<f32>(0.0, dv), 0.0).r) / (2.0 * dv);
            let bn = cross(dpdv, n);
            let tn = cross(n, dpdu);
            let d = dot(dpdu, bn);
            if (abs(d) > 1e-12) {
                let gr = (bn * hu + tn * hv) / d;
                n = normalize(n - gr * (it.extra.y * 0.02));
            }
        }
    }
    let metallic = it.params.x;
    let rough = clamp(it.params.y, 0.04, 1.0);
    let trans = it.params.z;
    let ior = it.params.w;
    let f0g = pow((ior - 1.0) / (ior + 1.0), 2.0);
    let f0d = 0.04 + (f0g - 0.04) * trans;
    let f0 = vec3<f32>(f0d) + (base.rgb - vec3<f32>(f0d)) * metallic;
    let kd = (1.0 - metallic) * (1.0 - trans);
    let diffuse = base.rgb * kd;
    let cc = it.extra.x;
    let cc_shin = lobe_shininess(0.06);
    let cc_norm = lobe_norm(0.06, cc_shin);
    var out = vec3<f32>(0.0);
    let r = n * (2.0 * dot(n, v)) - v;
    let count = i32(g.eye.w);
    for (var i = 0; i < count; i++) {
        let l = g.lights[i];
        let kind = i32(l.v.w + 0.5);
        var dir: vec3<f32>;
        var k = 1.0;
        var dist = 3.4e38;
        var sdir: vec3<f32>;
        if (kind == 0) {
            dir = -l.v.xyz;
            sdir = dir;
        } else if (kind == 3) {
            var up = vec3<f32>(0.0, 1.0, 0.0);
            if (abs(l.dir.y) > 0.99) {
                up = vec3<f32>(0.0, 0.0, 1.0);
            }
            let ax = normalize(cross(l.dir.xyz, up));
            let ay = normalize(cross(ax, l.dir.xyz));
            let hw = l.cone.z / 2.0;
            let hh = l.cone.w / 2.0;
            let local = p - l.v.xyz;
            let q = l.v.xyz + ax * clamp(dot(local, ax), -hw, hw) + ay * clamp(dot(local, ay), -hh, hh);
            let d = q - p;
            dist = max(length(d), 1e-4);
            dir = d / dist;
            k = max(dot(-dir, l.dir.xyz), 0.0);
            let denom = dot(r, l.dir.xyz);
            if (denom < -1e-4) {
                let tr = dot(l.v.xyz - p, l.dir.xyz) / denom;
                let hit = p + r * max(tr, 0.0) - l.v.xyz;
                sdir = normalize((l.v.xyz + ax * clamp(dot(hit, ax), -hw, hw) + ay * clamp(dot(hit, ay), -hh, hh)) - p);
            } else {
                sdir = dir;
            }
        } else {
            let d = l.v.xyz - p;
            dist = length(d);
            if (l.color.w > 0.0) {
                let f = clamp(1.0 - dist / l.color.w, 0.0, 1.0);
                k = f * f;
            }
            dir = normalize(d);
            sdir = dir;
        }
        if (kind == 2) {
            let c = dot(-dir, l.dir.xyz);
            k *= smooth01((c - l.cone.x) / max(l.cone.y - l.cone.x, 1e-4));
        }
        let ndl = max(dot(n, dir), 0.0);
        if (ndl <= 0.0 || k <= 0.0) {
            continue;
        }
        var spread: f32;
        if (kind == 0) {
            spread = l.cone.z * 0.5;
        } else if (kind == 3) {
            spread = 0.5 * max(l.cone.z, l.cone.w) / dist;
        } else {
            spread = 0.5 * l.cone.z / max(dist, 1e-4);
        }
        let rl = min(rough + spread, 1.0);
        let shininess = lobe_shininess(rl);
        let norm = lobe_norm(rl, shininess);
        let h = normalize(sdir + v);
        let ndh = max(dot(n, h), 0.0);
        let vdh = max(dot(v, h), 0.0);
        let fr = pow(1.0 - vdh, 5.0);
        let spec = norm * pow(ndh, shininess);
        let sh = shadow_lit(i, p, n);
        let coat = cc * (0.04 + 0.96 * fr);
        let coat_spec = cc_norm * pow(ndh, cc_shin);
        let fc = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * fr;
        let lobe = diffuse * (vec3<f32>(1.0) - fc) + fc * spec;
        out += (lobe * (1.0 - coat) + vec3<f32>(coat * coat_spec)) * l.color.rgb * ndl * k * sh;
    }
    let ndv = max(dot(n, v), 0.0);
    let gloss = (1.0 - rough) * (1.0 - rough);
    if (g.env.x > -0.5) {
        let irr = env_irradiance(n);
        let refl = env_reflect(r, rough);
        let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
        let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
        let rr = rough * c0 + c1;
        let a004 = min(rr.x * rr.x, exp2(-9.28 * ndv)) * rr.x + rr.y;
        let ea = -1.04 * a004 + rr.z;
        let eb = 1.04 * a004 + rr.w;
        out += diffuse * irr + refl * (f0 * ea + vec3<f32>(eb)) + it.emissive.rgb;
    } else {
        let up = n.y * 0.5 + 0.5;
        let rup = r.y * 0.5 + 0.5;
        let amb = g.ground.rgb + (g.sky.rgb - g.ground.rgb) * up;
        let env = (g.ground.rgb + (g.sky.rgb - g.ground.rgb) * rup) * 2.0;
        let fre = f0 + (max(vec3<f32>(1.0 - rough), f0) - f0) * pow(1.0 - ndv, 5.0) * gloss;
        out += diffuse * amb + env * fre * gloss + it.emissive.rgb;
    }
    if (cc > 0.0) {
        let fcc = cc * (0.04 + 0.96 * pow(1.0 - ndv, 5.0));
        out = out * (1.0 - fcc) + world_light(r, 0.06) * fcc;
    }
    var alpha = base.a;
    if (trans > 0.0) {
        let fv = f0g + (1.0 - f0g) * pow(1.0 - ndv, 5.0);
        let through = trans * (1.0 - fv);
        let eta = 1.0 / max(ior, 1.0);
        let cosi = ndv;
        let k2 = 1.0 - eta * eta * (1.0 - cosi * cosi);
        var refr = r;
        if (k2 > 0.0) {
            refr = normalize(-v * eta + n * (eta * cosi - sqrt(k2)));
        }
        let bent = world_light(refr, rough);
        let avg = (base.r + base.g + base.b) / 3.0;
        out += through * base.rgb * bent * 0.3;
        alpha = max(base.a * (1.0 - through * avg), 0.02);
    }
    if (g.fog_color.w > 0.5) {
        var k = clamp((length(g.eye.xyz - p) - g.fog.x) / max(g.fog.y - g.fog.x, 0.001), 0.0, 1.0);
        k = k * k * (3.0 - 2.0 * k);
        out = out + (g.fog_color.rgb - out) * k;
    }
    let keep = base.a / max(alpha, 1e-6);
    return finish(out * keep, alpha, false);
}

// The world behind everything: one triangle covering the frame.
@vertex
fn vs_back(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 1.0, 1.0);
}

@fragment
fn fs_back(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var d = g.forward.xyz;
    if (g.sky.w < 0.5) {
        let nx = pos.x / g.screen.x * 2.0 - 1.0;
        let ny = 1.0 - pos.y / g.screen.y * 2.0;
        let t = g.right.w;
        let aspect = g.screen.x / max(g.screen.y, 1.0);
        d = normalize(g.forward.xyz + g.right.xyz * (nx * t * aspect) + g.up.xyz * (ny * t));
    }
    return finish(env_radiance(d), 1.0, false);
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
