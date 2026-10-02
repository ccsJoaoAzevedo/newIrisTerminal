//! The picture a floating-logo screen saver can show instead of the XP logo:
//! a PNG, or a GIF that keeps its own frame timing.
//!
//! Decoded once and kept as textures, keyed by the path and the file's
//! modification time, so a file saved over while the saver runs is picked up
//! and one that has not changed is never read twice. The full-window saver
//! and the preview in its dialog share the one copy.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use egui::{ColorImage, Context, Id, TextureHandle, TextureOptions, Vec2};

use crate::i18n::{tr, tr1};

/// Larger than this on either side is scaled down on load. A saver never
/// needs more, and a long GIF at full size is all of its frames times
/// width times height times four bytes of video memory.
const MAX_SIDE: u32 = 1024;

/// More frames than this are dropped; the animation loops on those it kept.
const MAX_FRAMES: usize = 600;

/// A GIF that asks for less than this between frames gets [`SLOW_DEFAULT`]
/// instead, as every browser does: a delay of 0 or 1 is how old encoders said
/// "unspecified", and taken at its word it would ask for a frame every frame.
const FASTEST: Duration = Duration::from_millis(20);
const SLOW_DEFAULT: Duration = Duration::from_millis(100);

/// How often the file is looked at again for a change.
const RECHECK: Duration = Duration::from_secs(2);

/// One decoded frame and how long it stays up.
pub struct Frame {
    pub image: ColorImage,
    pub delay: Duration,
}

/// A loaded picture: one texture for a PNG, one per frame for a GIF.
pub struct Picture {
    frames: Vec<TextureHandle>,
    delays: Vec<Duration>,
    pub size: Vec2,
}

impl Picture {
    /// The texture showing `elapsed` after the picture first went up, and
    /// how long until it changes - `None` for a still.
    pub fn frame_at(&self, elapsed: Duration) -> (&TextureHandle, Option<Duration>) {
        if self.frames.len() < 2 {
            return (&self.frames[0], None);
        }
        let (index, next) = frame_at(&self.delays, elapsed);
        (&self.frames[index], Some(next))
    }
}

/// Which frame of a looping animation with these `delays` is up `elapsed`
/// into it, and how long it has left.
///
/// The second half is what the saver's repaint is scheduled by: a GIF at
/// 10 frames a second costs ten frames a second, not sixty.
pub fn frame_at(delays: &[Duration], elapsed: Duration) -> (usize, Duration) {
    let total: Duration = delays.iter().sum();
    if delays.is_empty() || total.is_zero() {
        return (0, Duration::MAX);
    }
    let mut into = Duration::from_nanos((elapsed.as_nanos() % total.as_nanos()) as u64);
    for (i, &delay) in delays.iter().enumerate() {
        if into < delay {
            return (i, delay - into);
        }
        into -= delay;
    }
    (delays.len() - 1, Duration::ZERO)
}

/// A GIF frame's delay as the file states it, made fit to schedule by.
pub fn frame_delay(stated: Duration) -> Duration {
    if stated < FASTEST {
        SLOW_DEFAULT
    } else {
        stated
    }
}

/// Decodes a PNG or a GIF. Anything else is refused by name: it may well be
/// an image, but it is not what the setting promises to show.
pub fn decode(bytes: &[u8]) -> Result<Vec<Frame>, String> {
    use image::AnimationDecoder;

    let format =
        image::guess_format(bytes).map_err(|_| tr("Not a PNG or GIF image.").to_owned())?;
    let broken = |e: image::ImageError| tr1("The image could not be read: {}", &e.to_string());
    let frames = match format {
        image::ImageFormat::Png => {
            let image = image::load_from_memory_with_format(bytes, format).map_err(broken)?;
            vec![(image.into_rgba8(), Duration::ZERO)]
        }
        image::ImageFormat::Gif => {
            let decoder =
                image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).map_err(broken)?;
            let mut frames = Vec::new();
            for frame in decoder.into_frames().take(MAX_FRAMES) {
                let frame = frame.map_err(broken)?;
                let delay = Duration::from(frame.delay());
                frames.push((frame.into_buffer(), frame_delay(delay)));
            }
            frames
        }
        _ => return Err(tr("Not a PNG or GIF image.").to_owned()),
    };
    if frames.is_empty() {
        return Err(tr("The image has nothing in it.").to_owned());
    }
    Ok(frames
        .into_iter()
        .map(|(rgba, delay)| {
            let rgba = shrunk(rgba);
            let size = [rgba.width() as usize, rgba.height() as usize];
            Frame {
                image: ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                delay,
            }
        })
        .collect())
}

fn shrunk(rgba: image::RgbaImage) -> image::RgbaImage {
    let (w, h) = rgba.dimensions();
    if w <= MAX_SIDE && h <= MAX_SIDE {
        return rgba;
    }
    let scale = MAX_SIDE as f32 / w.max(h) as f32;
    let (nw, nh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Triangle)
}

/// Reads and decodes the file at `path`.
pub fn load(path: &Path) -> Result<Vec<Frame>, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| tr1("The image could not be opened: {}", &e.to_string()))?;
    decode(&bytes)
}

/// Whether `path` names a file the saver would take, without decoding it -
/// for the settings dialog to say so as it is typed.
pub fn looks_like_picture(path: &str) -> bool {
    let lower = path.trim().to_lowercase();
    lower.ends_with(".png") || lower.ends_with(".gif")
}

#[derive(Default)]
struct Cache {
    path: PathBuf,
    modified: Option<SystemTime>,
    checked: Option<Instant>,
    picture: Option<Result<Arc<Picture>, String>>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The picture at `path`, loaded on first use and again when the file
/// changes; the reason it would not load otherwise.
///
/// Only the last path asked for is kept: there is one saver, and keeping the
/// textures of every file ever tried would keep their video memory too.
pub fn picture(ctx: &Context, path: &str) -> Result<Arc<Picture>, String> {
    let cache = ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(Id::new("nit-screensaver-picture"), || {
            Arc::new(Mutex::new(Cache::default()))
        })
        .clone()
    });
    let Ok(mut cache) = cache.lock() else {
        return Err(tr("The image could not be read.").to_owned());
    };
    let path = PathBuf::from(path.trim());
    let now = Instant::now();
    let due = cache.path != path
        || cache
            .checked
            .is_none_or(|at| now.duration_since(at) >= RECHECK);
    if due {
        let stamp = modified(&path);
        if cache.path != path || cache.modified != stamp || cache.picture.is_none() {
            cache.picture = Some(load(&path).map(|frames| Arc::new(upload(ctx, &path, frames))));
            cache.path = path;
            cache.modified = stamp;
        }
        cache.checked = Some(now);
    }
    cache
        .picture
        .clone()
        .unwrap_or_else(|| Err(tr("The image could not be read.").to_owned()))
}

fn upload(ctx: &Context, path: &Path, frames: Vec<Frame>) -> Picture {
    let first = &frames[0].image;
    let size = Vec2::new(first.width() as f32, first.height() as f32);
    let name = path.display().to_string();
    let mut delays = Vec::with_capacity(frames.len());
    let textures = frames
        .into_iter()
        .enumerate()
        .map(|(i, frame)| {
            delays.push(frame.delay);
            ctx.load_texture(format!("{name}#{i}"), frame.image, TextureOptions::LINEAR)
        })
        .collect();
    Picture {
        frames: textures,
        delays,
        size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn an_animation_shows_each_frame_for_its_own_delay_and_loops() {
        let delays = [ms(100), ms(300), ms(50)];
        assert_eq!(frame_at(&delays, ms(0)), (0, ms(100)));
        assert_eq!(frame_at(&delays, ms(99)), (0, ms(1)));
        assert_eq!(frame_at(&delays, ms(100)), (1, ms(300)));
        assert_eq!(frame_at(&delays, ms(380)), (1, ms(20)));
        assert_eq!(frame_at(&delays, ms(420)), (2, ms(30)));
        // Round again.
        assert_eq!(frame_at(&delays, ms(450)), (0, ms(100)));
        assert_eq!(frame_at(&delays, ms(450 * 7 + 120)), (1, ms(280)));
    }

    /// The next repaint is the frame's own end, so a slow GIF asks for few
    /// frames - and one with no timing at all does not ask for one per frame.
    #[test]
    fn a_gif_frame_that_asks_for_no_delay_gets_the_browsers_tenth_of_a_second() {
        assert_eq!(frame_delay(Duration::ZERO), ms(100));
        assert_eq!(frame_delay(ms(10)), ms(100));
        assert_eq!(frame_delay(ms(20)), ms(20));
        assert_eq!(frame_delay(ms(500)), ms(500));
        assert_eq!(
            frame_at(&[Duration::ZERO], ms(5)).0,
            0,
            "never divides by zero"
        );
    }

    fn encoded_png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(w, h, image::Rgba([200, 10, 10, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn a_png_is_one_still_frame_and_a_huge_one_is_scaled_down() {
        let frames = decode(&encoded_png(4, 3)).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].image.size, [4, 3]);
        let frames = decode(&encoded_png(3000, 1500)).unwrap();
        assert_eq!(frames[0].image.size, [1024, 512]);
    }

    #[test]
    fn a_gif_keeps_every_frame_with_its_delay() {
        let mut bytes = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
            for (shade, delay) in [(0u8, 70u32), (255, 250)] {
                let buffer =
                    image::RgbaImage::from_pixel(2, 2, image::Rgba([shade, shade, shade, 255]));
                let frame = image::Frame::from_parts(
                    buffer,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(delay, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        let frames = decode(&bytes).unwrap();
        let delays: Vec<Duration> = frames.iter().map(|f| f.delay).collect();
        assert_eq!(delays, vec![ms(70), ms(250)]);
    }

    #[test]
    fn a_file_that_is_not_a_png_or_gif_is_refused_with_a_reason() {
        assert!(decode(b"definitely not an image").is_err());
        // A real image, but not one of the two kinds promised.
        let mut bmp = Vec::new();
        image::RgbaImage::new(1, 1)
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        assert!(decode(&bmp).is_err());
        // Cut off halfway through.
        let png = encoded_png(8, 8);
        assert!(decode(&png[..png.len() / 2]).is_err());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let gone = std::env::temp_dir().join("nit-screensaver-test-no-such-file.png");
        let ctx = Context::default();
        let err = picture(&ctx, &gone.display().to_string()).err();
        assert!(err.is_some_and(|e| !e.is_empty()));
    }

    #[test]
    fn only_png_and_gif_names_look_like_pictures() {
        assert!(looks_like_picture("C:\\a\\Cat.GIF"));
        assert!(looks_like_picture(" logo.png "));
        assert!(!looks_like_picture("logo.jpg"));
        assert!(!looks_like_picture(""));
    }
}
