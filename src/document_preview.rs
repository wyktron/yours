//! Shared bounded file decoding used by Mail attachment dialogs and Files.
use std::sync::Arc;
// Cancelling spawn_blocking does not stop an active decoder. Keep admission
// inside the worker until decoding really ends, and never queue more images.
static IMAGE_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
static THUMBNAIL_SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
type Result<T> = std::result::Result<T, String>;
pub(crate) enum Preview {
    Text(String),
    Image(Vec<u8>, u32, u32),
    Pdf(Arc<Vec<u8>>, crate::pdf_preview::Page),
}
/// The type a preview should use: the stored type when it is specific, and
/// otherwise one inferred from the file name. Some senders store an empty or
/// generic type, so the name is then the only clue to what the bytes are.
/// Only the application types a preview can render count as specific; other
/// application types are often generic labels, so the name decides.
pub(crate) fn inferred_media(name: &str, media: &str) -> String {
    let media = media
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let specific = !media.is_empty()
        && media != "application/octet-stream"
        && (!media.starts_with("application/")
            || matches!(
                media.as_str(),
                "application/pdf" | "application/json" | "application/xml"
            ));
    if specific {
        return media;
    }
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let inferred = match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        "pdf" => "application/pdf",
        "txt" | "log" | "csv" => "text/plain",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "md" => "text/markdown",
        _ => return media,
    };
    inferred.to_string()
}
pub(crate) async fn preview(data: Vec<u8>, name: &str, media: &str) -> Result<Preview> {
    let media = inferred_media(name, media);
    let media = media.as_str();
    if media == "application/pdf" || data.starts_with(b"%PDF-") {
        let data = Arc::new(data);
        return crate::pdf_preview::render(data.clone(), 0, 200)
            .await
            .map(|page| Preview::Pdf(data, page));
    }
    if media.starts_with("image/") && media != "image/svg+xml" {
        if data.len() > 16 * 1024 * 1024 {
            return Ok(Preview::Text(
                "This image is too large to preview. Download it to view locally.".into(),
            ));
        }
        return decode_image(move || {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
                .with_guessed_format()
                .map_err(|e| e.to_string())?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader
                .decode()
                .map_err(|_| {
                    "Could not decode this image safely. Download it to view locally.".to_string()
                })?
                .thumbnail(1800, 1400)
                .into_rgba8();
            let (w, h) = image.dimensions();
            Ok(Preview::Image(image.into_raw(), w, h))
        })
        .await;
    }
    if (media.starts_with("text/") || matches!(media, "application/json" | "application/xml"))
        && data.len() <= 1024 * 1024
    {
        return Ok(Preview::Text(String::from_utf8_lossy(&data).into_owned()));
    }
    Ok(Preview::Text("Preview is available for PDF, images and text files. Download this file to view it in a local application.".into()))
}

async fn decode_image(work: impl FnOnce() -> Result<Preview> + Send + 'static) -> Result<Preview> {
    let permit = IMAGE_SLOT
        .try_acquire()
        .map_err(|_| "Another image is still being decoded. Try again shortly.".to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Decode a small attachment thumbnail on a separate, serialized lane so
/// background list decoration cannot make an explicitly requested preview
/// report that its decoder is busy.
pub(crate) async fn image_thumbnail(data: Vec<u8>) -> Result<(Vec<u8>, u32, u32)> {
    if data.len() > 4 * 1024 * 1024 {
        return Err("Image is too large for an attachment thumbnail".into());
    }
    let permit = THUMBNAIL_SLOT.acquire().await.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut reader = image::ImageReader::new(std::io::Cursor::new(data))
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|_| "Could not decode attachment thumbnail".to_string())?
            .thumbnail(96, 96)
            .into_rgba8();
        let (width, height) = image.dimensions();
        Ok((image.into_raw(), width, height))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    // Previews share one decoder slot, so tests that exercise it must not race.
    static IMAGE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn preview_with(name: &str, media: &str, bytes: &[u8]) -> Preview {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(preview(bytes.to_vec(), name, media))
            .unwrap()
    }

    #[test]
    fn attachment_thumbnail_is_bounded_and_preserves_aspect_ratio() {
        let source = image::RgbaImage::from_pixel(200, 100, image::Rgba([24, 120, 176, 255]));
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (pixels, width, height) = runtime
            .block_on(image_thumbnail(encoded.into_inner()))
            .unwrap();
        assert_eq!((width, height), (96, 48));
        assert_eq!(pixels.len(), (width * height * 4) as usize);
    }

    #[test]
    fn cancelling_a_preview_does_not_admit_another_decoder_until_the_worker_exits() {
        let _guard = IMAGE_TEST_LOCK.lock().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (started, wait_started) = tokio::sync::oneshot::channel();
            let (release, wait_release) = std::sync::mpsc::channel();
            let job = tokio::spawn(decode_image(move || {
                started.send(()).unwrap();
                wait_release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                Ok(Preview::Text("finished".into()))
            }));
            wait_started.await.unwrap();
            job.abort();
            assert!(job.await.err().unwrap().is_cancelled());
            assert!(
                decode_image(|| panic!("second decoder started"))
                    .await
                    .is_err()
            );
            release.send(()).unwrap();
            let permit =
                tokio::time::timeout(std::time::Duration::from_secs(5), IMAGE_SLOT.acquire())
                    .await
                    .unwrap()
                    .unwrap();
            drop(permit);
            assert!(
                decode_image(|| Ok(Preview::Text("next".into())))
                    .await
                    .is_ok()
            );
        });
    }

    #[test]
    fn a_jpeg_name_without_a_media_type_dispatches_to_the_image_decoder() {
        let _guard = IMAGE_TEST_LOCK.lock().unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([200, 40, 40]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Jpeg)
        .unwrap();
        assert!(matches!(
            preview_with("Webcam_2013-10-28_15-50-04.jpg", "", &encoded.into_inner()),
            Preview::Image(..)
        ));
    }

    #[test]
    fn html_with_a_generic_media_type_previews_as_plain_text() {
        let source = b"<html><body><script>alert(1)</script>Hello</body></html>";
        let Preview::Text(text) = preview_with("page.html", "application/octet-stream", source)
        else {
            panic!("HTML must preview as plain text");
        };
        assert_eq!(
            text,
            "<html><body><script>alert(1)</script>Hello</body></html>"
        );
    }

    #[test]
    fn a_stored_image_type_still_dispatches_to_the_image_decoder() {
        let _guard = IMAGE_TEST_LOCK.lock().unwrap();
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([24, 120, 176, 255]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        assert!(matches!(
            preview_with("attachment", "image/png", &encoded.into_inner()),
            Preview::Image(..)
        ));
    }
}
