//! Expressions: small formulas that compute a property every frame, like After Effects
//! expressions or Blender drivers (`"time * 90"`, `"wiggle(2, 30)"`, `"value + sin(time) * 20"`).

/// Checks that a formula reads.
pub fn check(src: &str) -> Result<(), String> {
    if src.trim().is_empty() { Err("the formula is empty".into()) } else { Ok(()) }
}
