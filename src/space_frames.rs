//! The Space preset's background frames and their native-size tile grid.
//!
//! The owner's installed Firefox theme ships `background.gif`: 640x360, 119
//! frames at 60-70 ms, a starfield that twinkles quickly and decorrelates over
//! a few frames. The app renders those pixels instead of imitating them. All
//! 119 frames are embedded here as lossless PNGs, so no external file is
//! needed at runtime.
//!
//! Decoding happens once at startup. Each frame is expanded into a mirrored
//! 1400x840 mosaic: a 1280x720 display block plus 120 device pixels of slack on
//! each axis. The extra room lets every tile take its own crop (origin jitter)
//! without scaling, and the mirror checkerboard means neighbouring copies meet
//! on a reflection axis instead of a hard repeat.
//!
//! `Starfield` shows a staggered grid of 1280x720 tiles, two layers deep. Every
//! tile picks its own crop, mirror flags, row offset and frame phase from a
//! fixed hash, so the field never lines up as a visible grid and the twinkle is
//! not synchronised across the window. One image pixel stays one device pixel:
//! the display block is `1280 / device-pixel-ratio` logical pixels wide, and
//! the source crop is exactly 1280x720 pixels. Rust rebuilds the grid when the
//! window's size or scale changes.

use crate::{AppWindow, SpaceTile};
use image::ImageEncoder;
use slint::{ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::rc::Rc;
use std::time::Instant;

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
const FRAME_WIDTH: u32 = 640;
const FRAME_HEIGHT: u32 = 360;
/// The display block: a 2x2 native copy of the reference frame.
pub(crate) const TILE_WIDTH: u32 = 1280;
pub(crate) const TILE_HEIGHT: u32 = 720;
/// Slack after the display block, so every tile can take its own crop.
/// The field is mostly black at these edges, so the extra pixels stay unseen.
const JITTER_X: u32 = 120;
const JITTER_Y: u32 = 120;
const MOSAIC_WIDTH: u32 = TILE_WIDTH + JITTER_X;
const MOSAIC_HEIGHT: u32 = TILE_HEIGHT + JITTER_Y;
/// Playback speed of the reference GIF.
pub(crate) const FRAME_PERIOD: std::time::Duration = std::time::Duration::from_millis(70);
/// Frame phases are spread over four slots. More slots would make more tiles
/// swap frames (and upload textures) on the same 70 ms tick; four keeps that
/// spike inside the frame budget while still decorrelating the shimmer.
const PHASE_SLOTS: usize = 4;
/// A fixed seed keeps the "random" arrangement identical across restarts.
const LAYOUT_SEED: u64 = 0x5eed_5ca7_7e11_1234;

/// Cache directory for the expanded mosaics. The name changes whenever the
/// mosaic geometry or content changes, so an older cache is never mixed in.
const CACHE_DIR: &str = "space-mosaics-v1-1400x840";

/// The static grid model. Keeping it separate from the timer state lets Rust
/// rebuild the tiles on resize without recreating the window property.
pub(crate) struct SpaceModels {
    pub(crate) tiles: Rc<VecModel<SpaceTile>>,
}

impl SpaceModels {
    pub(crate) fn new() -> Self {
        Self {
            tiles: Rc::new(VecModel::default()),
        }
    }

    pub(crate) fn install(&self, app: &AppWindow) {
        app.set_space_tiles(ModelRc::new(self.tiles.clone()));
    }
}

/// Decode every embedded frame in playback order, expanded to its tile mosaic.
///
/// The mosaics are written to a small PNG cache and loaded back from disk.
/// Slint can cache a path-backed image's GPU texture between frames, while a
/// programmatically built image has no cache key and is re-uploaded on every
/// draw. That difference is the whole frame budget: the whole grid used to
/// re-upload ~85 MB per frame, which capped the field at 30 fps.
pub(crate) fn load(cache_dir: &std::path::Path) -> Vec<Image> {
    let started = Instant::now();
    let directory = cache_dir.join(CACHE_DIR);
    let mut cached = 0usize;
    let mut generated = 0usize;
    let mut uncached = 0usize;
    let frames = FRAMES
        .iter()
        .enumerate()
        .map(|(index, png)| {
            let path = directory.join(format!("frame-{index:03}.png"));
            if let Ok(image) = Image::load_from_path(&path) {
                cached += 1;
                return image;
            }
            let decoded = image::load_from_memory(png)
                .expect("the embedded Space frames are valid PNGs")
                .to_rgba8();
            let (width, height) = decoded.dimensions();
            debug_assert_eq!((width, height), (FRAME_WIDTH, FRAME_HEIGHT));
            let buffer = tile_frame(decoded.as_raw(), index as u64);
            if write_cached(&directory, &path, &buffer).is_ok()
                && let Ok(image) = Image::load_from_path(&path)
            {
                generated += 1;
                return image;
            }
            // A read-only cache directory must not stop the field; the image
            // still looks right, it just gets re-uploaded by the renderer.
            uncached += 1;
            Image::from_rgba8(buffer)
        })
        .collect();
    eprintln!(
        "space frames: {} mosaics ({}x{}) in {:.2}s (cached {}, generated {}, uncached {})",
        FRAMES.len(),
        MOSAIC_WIDTH,
        MOSAIC_HEIGHT,
        started.elapsed().as_secs_f64(),
        cached,
        generated,
        uncached
    );
    frames
}

/// Write one mosaic as a fast PNG, then load it as a path-backed image.
fn write_cached(
    directory: &std::path::Path,
    path: &std::path::Path,
    buffer: &SharedPixelBuffer<Rgba8Pixel>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let temporary = path.with_extension("png.tmp");
    {
        let file = std::fs::File::create(&temporary)?;
        let writer = std::io::BufWriter::new(file);
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            writer,
            image::codecs::png::CompressionType::Fast,
            image::codecs::png::FilterType::NoFilter,
        );
        encoder
            .write_image(
                buffer.as_bytes(),
                MOSAIC_WIDTH,
                MOSAIC_HEIGHT,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(std::io::Error::other)?;
    }
    std::fs::rename(temporary, path)
}

/// Expand one decoded 640x360 frame into a mirrored, 1400x840 mosaic.
///
/// Copies alternate mirrors on both axes, so every seam is a reflection axis
/// rather than a repeat. The extra 120 px on each axis gives the tiles their
/// crop origin jitter.
fn tile_frame(source: &[u8], seed: u64) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(MOSAIC_WIDTH, MOSAIC_HEIGHT);
    let pixels = buffer.make_mut_slice();
    // Alternate mirrors on both axes, offset by a per-frame parity: every cell
    // boundary is then a reflection axis, so neighbouring copies meet without a
    // hard repeat and different frame phases cannot line their patterns up.
    let base_mirror = splitmix64(seed.wrapping_add(0x9e37_79b9_7f4a_7c15)) & 1 == 1;
    for y in 0..MOSAIC_HEIGHT {
        let row = y / FRAME_HEIGHT;
        let row_mirror = base_mirror ^ (row % 2 == 1);
        let sy = if row_mirror {
            FRAME_HEIGHT - 1 - (y % FRAME_HEIGHT)
        } else {
            y % FRAME_HEIGHT
        };
        for x in 0..MOSAIC_WIDTH {
            let column = x / FRAME_WIDTH;
            let column_mirror = row_mirror ^ (column % 2 == 1);
            let sx = if column_mirror {
                FRAME_WIDTH - 1 - (x % FRAME_WIDTH)
            } else {
                x % FRAME_WIDTH
            };
            let source_index = ((sy * FRAME_WIDTH + sx) * 4) as usize;
            pixels[(y * MOSAIC_WIDTH + x) as usize] = Rgba8Pixel {
                r: source[source_index],
                g: source[source_index + 1],
                b: source[source_index + 2],
                a: source[source_index + 3],
            };
        }
    }
    buffer
}

/// Rebuild the tile grid for a window of `width` x `height` logical pixels.
///
/// Every row is shifted by its own deterministic amount, so the vertical seams
/// of the grid never line up across rows. Within a row the tiles are exactly
/// one display block apart, and each tile gets its own crop, mirror flags and
/// frame phase from the same hash.
pub(crate) fn arrange_tiles(app: &AppWindow, width: f32, height: f32, models: &SpaceModels) {
    let scale = app.window().scale_factor();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return;
    }
    let tile_width = TILE_WIDTH as f32 / scale;
    let tile_height = TILE_HEIGHT as f32 / scale;
    let rows = (height / tile_height).ceil().max(1.0) as u32;
    let frame_count = FRAMES.len();
    let mut phase_slots_used = 0u16;
    let mut tiles = Vec::new();
    for row in 0..rows {
        // A per-row horizontal shift breaks the alignment of vertical seams.
        // The first tile starts at `shift - tile_width` so the left edge stays
        // covered without scaling the image.
        let row_hash = splitmix64(LAYOUT_SEED ^ ((row as u64) << 32));
        let shift = (row_hash % TILE_WIDTH as u64) as f32 / scale;
        let start = if shift > 0.0 {
            shift - tile_width
        } else {
            0.0
        };
        let mut x = start;
        let mut column = 0u32;
        while x < width {
            let column_hash = splitmix64(
                LAYOUT_SEED
                    ^ ((row as u64) << 32)
                    ^ ((column as u64) << 16)
                    ^ 0x51ed,
            );
            let clip_x = (column_hash % (JITTER_X as u64 + 1)) as i32;
            let clip_y = ((column_hash >> 16) % (JITTER_Y as u64 + 1)) as i32;
            let mirror_x = ((column_hash >> 32) & 1) == 1;
            let mirror_y = ((column_hash >> 33) & 1) == 1;
            let preferred = (((column_hash >> 40) as u32) % PHASE_SLOTS as u32) as u16;
            // Keep the tiles on distinct frame phases where possible, so the
            // twinkle is not synchronised across the screen.
            let phase_slot = if phase_slots_used & (1 << preferred) == 0 {
                preferred
            } else {
                (0..PHASE_SLOTS as u16)
                    .find(|slot| phase_slots_used & (1 << slot) == 0)
                    .unwrap_or(preferred)
            };
            phase_slots_used |= 1 << phase_slot;
            let phase = phase_slot as i32 * (frame_count / PHASE_SLOTS) as i32;
            tiles.push(SpaceTile {
                x,
                y: row as f32 * tile_height,
                phase,
                clip_x,
                clip_y,
                mirror_x,
                mirror_y,
            });
            x += tile_width;
            column += 1;
        }
    }
    models.tiles.set_vec(tiles);
    app.set_space_pixel_ratio(scale);
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut mixed = value;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^ (mixed >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_whole_reference_frame_set() {
        let directory = tempfile::tempdir().unwrap();
        let frames = load(directory.path());
        assert_eq!(frames.len(), FRAMES.len());
        assert_eq!(FRAMES.len(), 119);
        for frame in frames {
            assert_eq!(
                (frame.size().width, frame.size().height),
                (MOSAIC_WIDTH, MOSAIC_HEIGHT)
            );
        }
    }

    #[test]
    fn arranges_enough_native_tiles_to_cover_the_window() {
        // Guards the arithmetic: at 2.25x a 1320x800 window needs 3x3 blocks of
        // 1280x720 physical pixels, at 1x a 1920x1080 window needs 3x3 too.
        for (scale, width, height) in [(2.25, 1320.0, 800.0), (1.0, 1920.0, 1080.0)] {
            let (tile_width, tile_height) =
                (TILE_WIDTH as f32 / scale, TILE_HEIGHT as f32 / scale);
            assert!(tile_width * 2.0 < width + TILE_WIDTH);
            assert_eq!((height / tile_height).ceil() as i32, 3);
        }
    }

    #[test]
    fn phases_are_distinct_across_the_grid() {
        let mut seen = Vec::new();
        for row in 0..3u32 {
            for column in 0..3u32 {
                let hash = splitmix64(
                    LAYOUT_SEED ^ ((row as u64) << 32) ^ ((column as u64) << 16) ^ 0x51ed,
                );
                let slot = ((hash >> 40) as u32) % PHASE_SLOTS as u32;
                seen.push(slot);
            }
        }
        seen.sort_unstable();
        seen.dedup();
        assert!(seen.len() >= 3, "tile phases should use most of the four slots");
    }
}
