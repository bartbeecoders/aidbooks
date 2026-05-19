//! AidBooks-flavored defaults for mold. Moved out of the backend so the
//! service owns the policy and the API just sends `is_short` + `model`.

/// Default mold model when nobody picked one. `flux2-klein:q8` is
/// upstream's default — fast, low VRAM, decent quality.
pub const DEFAULT_MOLD_MODEL: &str = "flux2-klein:q8";

/// Translate `is_short` into mold's `width`/`height`. Mold requires
/// multiples of 16. For shorts we use 768×1360 — 16-aligned, ~9:16
/// (0.565 vs 0.5625), and 1.04MP. We deliberately match the megapixel
/// count of the square 1024×1024 default because that's the largest
/// resolution flux2-klein:q8 reliably fits on a 16GB GPU; at 1008×1776
/// (1.79MP) the transformer's attention activations OOM. The publisher
/// up-scales to the final 1080×1920 frame anyway.
pub fn dimensions_for(is_short: bool) -> (u32, u32) {
    if is_short {
        (768, 1360)
    } else {
        (1024, 1024)
    }
}

/// Reasonable step count for a given mold model slug. Distilled /
/// turbo / schnell / klein models converge in ~4 steps; flagship dev
/// models and CFG-based SDXL want ~25. Pattern-matches the slug
/// instead of carrying a hand-tuned table because mold's catalog keeps
/// growing — when in doubt, 20 is a safe middle ground.
pub fn default_steps_for(model: &str) -> u32 {
    let m = model.to_ascii_lowercase();
    if m.contains("schnell")
        || m.contains("klein")
        || m.contains("turbo")
        || m.contains("distilled")
    {
        4
    } else if m.contains("z-image") {
        9
    } else if m.contains("dev") || m.contains("sdxl") || m.contains("qwen-image") {
        25
    } else {
        20
    }
}

/// CFG guidance default per family. Distilled / non-CFG models want
/// 0.0; CFG-based families want ~3.5. Returns `None` to let mold's own
/// server-side `default_guidance` kick in.
pub fn default_guidance_for(model: &str) -> Option<f64> {
    let m = model.to_ascii_lowercase();
    if m.contains("schnell") || m.contains("klein") || m.contains("z-image") {
        Some(0.0)
    } else {
        None
    }
}

/// `true` when an upstream error string looks like a CUDA OOM. We hold
/// the semaphore past mold's degrade cooldown when this fires.
pub fn is_oom_error(msg: &str) -> bool {
    msg.contains("OUT_OF_MEMORY") || msg.contains("out of memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_are_multiples_of_16() {
        for is_short in [false, true] {
            let (w, h) = dimensions_for(is_short);
            assert_eq!(w % 16, 0, "width must be multiple of 16: {w}");
            assert_eq!(h % 16, 0, "height must be multiple of 16: {h}");
        }
    }

    #[test]
    fn steps_match_family() {
        assert_eq!(default_steps_for("flux2-klein:q8"), 4);
        assert_eq!(default_steps_for("flux-schnell"), 4);
        assert_eq!(default_steps_for("ltx-video-0.9.6-distilled:bf16"), 4);
        assert_eq!(default_steps_for("flux-dev:q4"), 25);
        assert_eq!(default_steps_for("sdxl:base"), 25);
        assert_eq!(default_steps_for("qwen-image:q2"), 25);
        assert_eq!(default_steps_for("z-image"), 9);
        assert_eq!(default_steps_for("something-else"), 20);
    }

    #[test]
    fn guidance_zero_for_distilled() {
        assert_eq!(default_guidance_for("flux2-klein:q8"), Some(0.0));
        assert_eq!(default_guidance_for("flux-schnell"), Some(0.0));
        assert_eq!(default_guidance_for("z-image"), Some(0.0));
        assert_eq!(default_guidance_for("flux-dev:q4"), None);
        assert_eq!(default_guidance_for("sdxl:base"), None);
    }

    #[test]
    fn oom_detection() {
        assert!(is_oom_error("CUDA OUT_OF_MEMORY"));
        assert!(is_oom_error("torch: out of memory on device 0"));
        assert!(!is_oom_error("rate limited"));
    }
}
