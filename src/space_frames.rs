//! The Space preset's background frames and their native-size tile grid.
//!
//! The owner's installed Firefox theme ships `background.gif`: 640x360, 119
//! frames at 60-70 ms, a starfield that twinkles quickly and decorrelates over
//! a few frames. The app renders those pixels instead of imitating them. All
//! 119 frames are embedded here as lossless PNGs, so no external file is
//! needed at runtime.
//!
//! Decoding happens once at startup. 119 RGBA frames of 640x360 are 109.7 MB,
//! inside the 120 MB budget; every frame is kept, so the loop runs the whole
//! 8.3 s reference at 70 ms a step. See `AppWindow.advance-space-frame`.
//!
//! `Starfield` tiles the current frame at native size. The GIF's 1-px stars
//! stay one device pixel instead of being stretched over the window: a tile is
//! `640 / device-pixel-ratio` logical pixels wide, so the renderer maps it to
//! exactly 640 physical pixels, and the tiles sit at exact multiples of that
//! size. The grid holds just enough tiles to cover the window, rebuilt when the
//! window's size or scale changes.

use crate::{AppWindow, SpaceTile};
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

/// The reference frames in playback order, embedded in the binary.
const FRAMES: [&[u8]; 119] = [
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
    include_bytes!("../resources/space-frames/space-024.png"),
    include_bytes!("../resources/space-frames/space-025.png"),
    include_bytes!("../resources/space-frames/space-026.png"),
    include_bytes!("../resources/space-frames/space-027.png"),
    include_bytes!("../resources/space-frames/space-028.png"),
    include_bytes!("../resources/space-frames/space-029.png"),
    include_bytes!("../resources/space-frames/space-030.png"),
    include_bytes!("../resources/space-frames/space-031.png"),
    include_bytes!("../resources/space-frames/space-032.png"),
    include_bytes!("../resources/space-frames/space-033.png"),
    include_bytes!("../resources/space-frames/space-034.png"),
    include_bytes!("../resources/space-frames/space-035.png"),
    include_bytes!("../resources/space-frames/space-036.png"),
    include_bytes!("../resources/space-frames/space-037.png"),
    include_bytes!("../resources/space-frames/space-038.png"),
    include_bytes!("../resources/space-frames/space-039.png"),
    include_bytes!("../resources/space-frames/space-040.png"),
    include_bytes!("../resources/space-frames/space-041.png"),
    include_bytes!("../resources/space-frames/space-042.png"),
    include_bytes!("../resources/space-frames/space-043.png"),
    include_bytes!("../resources/space-frames/space-044.png"),
    include_bytes!("../resources/space-frames/space-045.png"),
    include_bytes!("../resources/space-frames/space-046.png"),
    include_bytes!("../resources/space-frames/space-047.png"),
    include_bytes!("../resources/space-frames/space-048.png"),
    include_bytes!("../resources/space-frames/space-049.png"),
    include_bytes!("../resources/space-frames/space-050.png"),
    include_bytes!("../resources/space-frames/space-051.png"),
    include_bytes!("../resources/space-frames/space-052.png"),
    include_bytes!("../resources/space-frames/space-053.png"),
    include_bytes!("../resources/space-frames/space-054.png"),
    include_bytes!("../resources/space-frames/space-055.png"),
    include_bytes!("../resources/space-frames/space-056.png"),
    include_bytes!("../resources/space-frames/space-057.png"),
    include_bytes!("../resources/space-frames/space-058.png"),
    include_bytes!("../resources/space-frames/space-059.png"),
    include_bytes!("../resources/space-frames/space-060.png"),
    include_bytes!("../resources/space-frames/space-061.png"),
    include_bytes!("../resources/space-frames/space-062.png"),
    include_bytes!("../resources/space-frames/space-063.png"),
    include_bytes!("../resources/space-frames/space-064.png"),
    include_bytes!("../resources/space-frames/space-065.png"),
    include_bytes!("../resources/space-frames/space-066.png"),
    include_bytes!("../resources/space-frames/space-067.png"),
    include_bytes!("../resources/space-frames/space-068.png"),
    include_bytes!("../resources/space-frames/space-069.png"),
    include_bytes!("../resources/space-frames/space-070.png"),
    include_bytes!("../resources/space-frames/space-071.png"),
    include_bytes!("../resources/space-frames/space-072.png"),
    include_bytes!("../resources/space-frames/space-073.png"),
    include_bytes!("../resources/space-frames/space-074.png"),
    include_bytes!("../resources/space-frames/space-075.png"),
    include_bytes!("../resources/space-frames/space-076.png"),
    include_bytes!("../resources/space-frames/space-077.png"),
    include_bytes!("../resources/space-frames/space-078.png"),
    include_bytes!("../resources/space-frames/space-079.png"),
    include_bytes!("../resources/space-frames/space-080.png"),
    include_bytes!("../resources/space-frames/space-081.png"),
    include_bytes!("../resources/space-frames/space-082.png"),
    include_bytes!("../resources/space-frames/space-083.png"),
    include_bytes!("../resources/space-frames/space-084.png"),
    include_bytes!("../resources/space-frames/space-085.png"),
    include_bytes!("../resources/space-frames/space-086.png"),
    include_bytes!("../resources/space-frames/space-087.png"),
    include_bytes!("../resources/space-frames/space-088.png"),
    include_bytes!("../resources/space-frames/space-089.png"),
    include_bytes!("../resources/space-frames/space-090.png"),
    include_bytes!("../resources/space-frames/space-091.png"),
    include_bytes!("../resources/space-frames/space-092.png"),
    include_bytes!("../resources/space-frames/space-093.png"),
    include_bytes!("../resources/space-frames/space-094.png"),
    include_bytes!("../resources/space-frames/space-095.png"),
    include_bytes!("../resources/space-frames/space-096.png"),
    include_bytes!("../resources/space-frames/space-097.png"),
    include_bytes!("../resources/space-frames/space-098.png"),
    include_bytes!("../resources/space-frames/space-099.png"),
    include_bytes!("../resources/space-frames/space-100.png"),
    include_bytes!("../resources/space-frames/space-101.png"),
    include_bytes!("../resources/space-frames/space-102.png"),
    include_bytes!("../resources/space-frames/space-103.png"),
    include_bytes!("../resources/space-frames/space-104.png"),
    include_bytes!("../resources/space-frames/space-105.png"),
    include_bytes!("../resources/space-frames/space-106.png"),
    include_bytes!("../resources/space-frames/space-107.png"),
    include_bytes!("../resources/space-frames/space-108.png"),
    include_bytes!("../resources/space-frames/space-109.png"),
    include_bytes!("../resources/space-frames/space-110.png"),
    include_bytes!("../resources/space-frames/space-111.png"),
    include_bytes!("../resources/space-frames/space-112.png"),
    include_bytes!("../resources/space-frames/space-113.png"),
    include_bytes!("../resources/space-frames/space-114.png"),
    include_bytes!("../resources/space-frames/space-115.png"),
    include_bytes!("../resources/space-frames/space-116.png"),
    include_bytes!("../resources/space-frames/space-117.png"),
    include_bytes!("../resources/space-frames/space-118.png"),
];

/// The reference frame's pixel size; one image pixel is one physical pixel.
const FRAME_WIDTH: f32 = 640.0;
const FRAME_HEIGHT: f32 = 360.0;

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

/// Rebuild the tile grid for a window of `width` x `height` logical pixels.
///
/// The reference frame is drawn at one image pixel per device pixel, so the
/// tile is `FRAME_WIDTH / scale_factor` logical pixels wide. The grid has
/// exactly `ceil(width / tile_width)` columns and `ceil(height / tile_height)`
/// rows, positioned at exact multiples so adjacent tiles meet without a seam.
pub(crate) fn arrange_tiles(app: &AppWindow, width: f32, height: f32) {
    let scale = app.window().scale_factor();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return;
    }
    let tile_width = FRAME_WIDTH / scale;
    let tile_height = FRAME_HEIGHT / scale;
    let columns = (width / tile_width).ceil().max(1.0) as i32;
    let rows = (height / tile_height).ceil().max(1.0) as i32;
    app.set_space_pixel_ratio(scale);
    let tiles = (0..rows)
        .flat_map(|row| {
            (0..columns).map(move |column| SpaceTile {
                x: column as f32 * tile_width,
                y: row as f32 * tile_height,
            })
        })
        .collect::<Vec<_>>();
    app.set_space_tiles(ModelRc::new(VecModel::from(tiles)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_whole_reference_frame_set() {
        let frames = load();
        assert_eq!(frames.len(), 119);
        for frame in frames {
            assert_eq!(
                (frame.size().width, frame.size().height),
                (FRAME_WIDTH as u32, FRAME_HEIGHT as u32)
            );
        }
    }

    #[test]
    fn arranges_enough_native_tiles_to_cover_the_window() {
        // Guards the arithmetic: at 2.25x a 1320x800 window needs 5x5 tiles of
        // 640x360 physical pixels, at 1x a 1920x1080 window needs 3x3.
        let scale = 2.25;
        let (width, height) = (1320.0, 800.0);
        let (tile_width, tile_height) = (FRAME_WIDTH / scale, FRAME_HEIGHT / scale);
        assert_eq!((width / tile_width).ceil() as i32, 5);
        assert_eq!((height / tile_height).ceil() as i32, 5);
        let scale = 1.0;
        let (width, height) = (1920.0, 1080.0);
        let (tile_width, tile_height) = (FRAME_WIDTH / scale, FRAME_HEIGHT / scale);
        assert_eq!((width / tile_width).ceil() as i32, 3);
        assert_eq!((height / tile_height).ceil() as i32, 3);
    }
}
