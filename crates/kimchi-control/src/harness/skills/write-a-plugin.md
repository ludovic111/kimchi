# Writing a plugin
When: the person wants an effect, a generator or a transition kimchi doesn't have, written as a video plugin (Rust, lsuite plugin SDK).

## Steps

1. Check what exists first: `plugin.list` (stock and installed effects, frei0r filters) and the built-in
   effects (`clip.setEffects`, `transition.kinds`, `motion.stackTypes`). Use one if it does the job.
2. `plugin.guide` is the SDK reference (the ABI, the rules, the templates): read it. The plugin permission
   must be on (Settings › Agent › Permissions › Plugins); a refusal says so.
3. `plugin.toolchain`: Rust must be installed. Missing: tell the person how to install it; never install
   it yourself.
4. `plugin.new {name, kind: "effect" | "generator" | "transition", description}` makes the crate from the
   template (in ~/.lsuite/plugins-src/kimchi/<name>/); never change its `id` later.
5. Write the code: `plugin.writeSource {name, path: "src/lib.rs", contents}` (only inside the crate).
   Keep it simple, per-pixel and allocation-free in the render call; parameters with sensible ranges and
   defaults.
6. `plugin.build {name}` until it builds: errors come back as `{file, line, column, message}`; fix them
   and build again.
7. `plugin.publishLocal {name}` installs and loads it at once (it replaces an earlier build).
8. Try it: `clip.addPlugin {clipIds, plugin, params}` on a clip (or `transition.set {plugin}` for a
   transition), then look.

## Checks

- `project.renderFrame` at a time inside the clip, with and without the plugin (`clip.setPlugin
  {clipId, slot, bypass: true}`): the effect shows and the picture isn't blank.
- Parameters change the result: set one to its extremes and look again.
- Report the plugin's name, its parameters and where it is installed.
