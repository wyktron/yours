//! The Space preset's literal reference frames and their native-size tile grid.
//!
//! The installed Firefox theme ships only a manifest plus `background.gif`:
//! 640x360, 119 frames at 60-70 ms, a starfield that twinkles quickly. The app
//! renders those exact pixels, embedded here as lossless PNGs; no star is
//! generated or imitated. All 119 frames are decoded once at startup.
//!
//! `Starfield` tiles the current frame at native size. One image pixel is one
//! device pixel: a tile is `640 / device-pixel-ratio` logical pixels wide, the
//! tiles sit at exact multiples, and there are no mirrors, flips, rotations or
//! per-tile crops. The frame repeats across the window, but every star is the
//! reference's own star. Two tile layers cross-fade consecutive frames from one
//! Slint animation clock.

use crate::{AppWindow, SpaceTile};
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::path::Path;

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
/// Cache directory for the literal frames. Slint caches a path-backed image's
/// GPU texture, while a programmatic image has no cache key and is re-uploaded
/// on every draw; writing the embedded PNG bytes once is what keeps the native
/// tiles to one texture per frame.
const CACHE_DIR: &str = "space-frames-v1-640x360";

/// Cache every embedded frame as a file, then load it as a path-backed image.
pub(crate) fn load(cache_dir: &Path) -> Vec<Image> {
    let directory = cache_dir.join(CACHE_DIR);
    if let Err(error) = std::fs::create_dir_all(&directory) {
        eprintln!("space frame cache unavailable: {error}");
    }
    FRAMES
        .iter()
        .enumerate()
        .map(|(index, png)| {
            let path = directory.join(format!("frame-{index:03}.png"));
            let cached = std::fs::metadata(&path)
                .map(|metadata| metadata.len() == png.len() as u64)
                .unwrap_or(false);
            if !cached {
                let _ = write_cached(&path, png);
            }
            Image::load_from_path(&path).unwrap_or_else(|_| {
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
        })
        .collect()
}

fn write_cached(path: &Path, png: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension("png.tmp");
    std::fs::write(&temporary, png)?;
    std::fs::rename(temporary, path)
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
    let columns = (width / tile_width).ceil().max(1.0) as u32;
    let rows = (height / tile_height).ceil().max(1.0) as u32;
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
        let directory = tempfile::tempdir().unwrap();
        let frames = load(directory.path());
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
        for (scale, width, height, expected) in
            [(2.25, 1320.0, 800.0, 5), (1.0, 1920.0, 1080.0, 3)]
        {
            let tile_width = FRAME_WIDTH / scale;
            let tile_height = FRAME_HEIGHT / scale;
            assert_eq!((width / tile_width).ceil() as i32, expected);
            assert_eq!((height / tile_height).ceil() as i32, expected);
        }
    }
}
