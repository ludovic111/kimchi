# 3D product shot
When: the person wants a 3D shot: a product turntable, a 3D title or logo, an abstract 3D backdrop, a model shown off by a camera move.

## Steps

1. Read `motion.guide {topic: "3d"}` (scene format: camera, lights, world, objects, materials).
2. Start from a template when one is close: `motion.addTemplate {template: "turntable", values: {model:
   "/path/product.glb"}}` (a glTF/GLB model on a pedestal), `title3d {text}`, `logoSpin3d {text | image}`,
   `shapes3d`. Otherwise `motion.add {scene: {type: "3d", camera, lights, objects}, start}`.
3. Light it like a studio: a key light at 45° above and to the side, a softer fill on the other side, a
   rim behind to separate it from the background; a floor plane to catch the shadow when it stands on
   something; a background that makes the product stand out.
4. Materials with intent: `metallic` and `roughness` (brushed metal 0.9/0.35, plastic 0/0.5, glass-like
   low roughness), `emissive` for glowing accents. `motion.setMaterial` changes one.
5. Camera: one purposeful move over the whole shot: `motion.cameraMove {clipId, move: "orbit" | "turntable"
   | "dolly" | "crane", degrees, from, to}`, easeInOut, slow (a 6 s shot turns 60–180°, not several
   turns). The product stays framed with some air around it.
6. Final quality: the standard engine renders the preview and the export; the path tracer (`motion.render`)
   is slow on a CPU: offer it, don't start it unasked.

## Checks

- `project.renderFrame {times: [start + 0.5, middle, end - 0.5]}`: the product is in frame and lit at all
  three, nothing passes through the camera, the shadow lands on the floor, no black frame (a light missing
  or the camera inside an object).
- `motion.view` shows the scene from any side when the framing puzzles you.
