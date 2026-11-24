//! The Space preset's background frames.
//!
//! The owner's installed Firefox theme ships `background.gif`: 640x360, 119
//! frames at 60-70 ms, a starfield that twinkles quickly and decorrelates over
//! a few frames. The app renders those pixels instead of imitating them.
//! Twenty-four frames are sampled evenly across the loop (indices 0, 5, ...,
//! 113, 118 of the source) and embedded here as lossless PNGs, so no external
//! file is needed at runtime.
//!
//! Decoding happens once at startup. The window timer advances through the
//! images every 70 ms; see `AppWindow.advance-space-frame`.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// The reference frames in playback order, embedded in the binary.
const FRAMES: [&[u8]; 24] = [
    include_bytes!("../resources/space-frames/space-000.png"),
    include_bytes!("../resources/space-frames/space-001.png"),
    include_bytes!("../resources/space-frames/space-002.png"),
    include_bytes!("../resources/space-frames/space-003.png"),
    include_bytes!("../resources/space-frames/space-004.png"),
    include_bytes!("../resources/space-frames/space-005.png"),
    include_bytes!("../resources/space-frames/space-006.png"),
    include_bytes!("../resources/space-frames/space-007.png"),
    include_bytes!("../resources/space-frames/space-008.png"),
    include_bytes!("../resources/space-frames/space-009.png"),
    include_bytes!("../resources/space-frames/space-010.png"),
    include_bytes!("../resources/space-frames/space-011.png"),
    include_bytes!("../resources/space-frames/space-012.png"),
    include_bytes!("../resources/space-frames/space-013.png"),
    include_bytes!("../resources/space-frames/space-014.png"),
    include_bytes!("../resources/space-frames/space-015.png"),
    include_bytes!("../resources/space-frames/space-016.png"),
    include_bytes!("../resources/space-frames/space-017.png"),
    include_bytes!("../resources/space-frames/space-018.png"),
    include_bytes!("../resources/space-frames/space-019.png"),
    include_bytes!("../resources/space-frames/space-020.png"),
    include_bytes!("../resources/space-frames/space-021.png"),
    include_bytes!("../resources/space-frames/space-022.png"),
    include_bytes!("../resources/space-frames/space-023.png"),
];

/// Decode every embedded frame in playback order.
pub(crate) fn load() -> Vec<Image> {
    FRAMES
        .iter()
        .map(|png| {
            let decoded = image::load_from_memory(png)
                .expect("the embedded Space frames are valid PNGs")
                .to_rgba8();
            let (width, height) = decoded.dimensions();
            Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                decoded.as_raw(),
                width,
                height,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_reference_frame_set() {
        let frames = load();
        assert_eq!(frames.len(), 24);
        for frame in frames {
            assert_eq!((frame.size().width, frame.size().height), (640, 360));
        }
    }
}
