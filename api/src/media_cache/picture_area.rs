//! Where a file's picture actually sits inside its frame: black bars baked into the encode
//! (a scope film inside 1920x1080, 4:3 inside 16:9) read from the trickplay thumbnails the
//! analyze pipeline already wrote, so nothing is decoded for it.

use crate::repositories::file_pictures::{self, PictureCheckFile};
use crate::repositories::{episode_skip_segments, movie_skip_segments};
use crate::streaming::session::MediaType;
use image::GrayImage;
use std::path::Path;
use uuid::Uuid;

/// Brighter than this is picture. A baked-in bar is true black — its thumbnails measure 0–2
/// — while a dark set, a stage or a night sky sits around 7–11, and must not read as bars.
const LIT_LUMA: u8 = 6;
/// A row or column is picture once this much of it is lit, so a channel logo or a
/// subtitle sitting in a bar does not count as picture.
const LIT_SHARE: f64 = 0.2;
/// A frame this dark overall cannot tell a bar from a night scene.
const DARK_MEAN_LUMA: f64 = 20.0;
/// Each edge takes the bar that at least nine in ten frames have: a film that opens up to
/// a taller ratio for some scenes reads as having no bar there, rather than being cut.
const BAR_PERCENTILE: f64 = 0.1;
/// Thinner than this is the edge of the encode, not a bar.
const MIN_BAR: f64 = 0.02;
const MIN_FRAMES: usize = 12;

/// Fractions of the frame, so they hold at any resolution the file is played at.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct PictureArea {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl PictureArea {
    pub const FULL: PictureArea = PictureArea {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };
}

/// `None` when too few frames could be read to say; [`PictureArea::FULL`] when there are no bars.
pub fn detect(frames: &[(f64, GrayImage)], exclude: &[(f64, f64)]) -> Option<PictureArea> {
    let insets: Vec<[f64; 4]> = frames
        .iter()
        .filter(|(at, _)| {
            !exclude
                .iter()
                .any(|(start, end)| (*start..*end).contains(at))
        })
        .filter_map(|(_, frame)| frame_insets(frame))
        .collect();
    if insets.len() < MIN_FRAMES {
        return None;
    }
    let edge = |i: usize| {
        let mut values: Vec<f64> = insets.iter().map(|e| e[i]).collect();
        values.sort_by(f64::total_cmp);
        let bar = values[(values.len() as f64 * BAR_PERCENTILE) as usize];
        if bar < MIN_BAR { 0.0 } else { bar }
    };
    let [top, bottom, left, right] = [edge(0), edge(1), edge(2), edge(3)];
    Some(PictureArea {
        x: left,
        y: top,
        width: 1.0 - left - right,
        height: 1.0 - top - bottom,
    })
}

/// Top, bottom, left and right bars of one frame as fractions of it.
fn frame_insets(frame: &GrayImage) -> Option<[f64; 4]> {
    let (w, h) = frame.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let mean = frame.pixels().map(|p| f64::from(p.0[0])).sum::<f64>() / f64::from(w * h);
    if mean < DARK_MEAN_LUMA {
        return None;
    }
    let lit = |x: u32, y: u32| frame.get_pixel(x, y).0[0] > LIT_LUMA;
    let row_lit = |y: u32| {
        f64::from((0..w).filter(|&x| lit(x, y)).count() as u32) >= f64::from(w) * LIT_SHARE
    };
    let col_lit = |x: u32| {
        f64::from((0..h).filter(|&y| lit(x, y)).count() as u32) >= f64::from(h) * LIT_SHARE
    };
    let top = (0..h).find(|&y| row_lit(y))?;
    let bottom = (0..h).rev().find(|&y| row_lit(y))?;
    let left = (0..w).find(|&x| col_lit(x))?;
    let right = (0..w).rev().find(|&x| col_lit(x))?;
    Some([
        f64::from(top) / f64::from(h),
        f64::from(h - 1 - bottom) / f64::from(h),
        f64::from(left) / f64::from(w),
        f64::from(w - 1 - right) / f64::from(w),
    ])
}

/// Reads a file's trickplay, leaving out its intro and credits ranges when they are known —
/// those are often framed differently from what they wrap — and records what it finds.
pub async fn check_file(
    pool: &sqlx::PgPool,
    storage_root: &str,
    media_type: MediaType,
    file_id: Uuid,
) -> crate::repositories::Result<Option<PictureArea>> {
    let exclude: Vec<(f64, f64)> = match media_type {
        MediaType::Movie => movie_skip_segments::list_by_movie_file(pool, file_id)
            .await?
            .into_iter()
            .map(|s| (s.start_seconds, s.end_seconds))
            .collect(),
        MediaType::Episode => episode_skip_segments::list_by_episode_file(pool, file_id)
            .await?
            .into_iter()
            .map(|s| (s.start_seconds, s.end_seconds))
            .collect(),
        MediaType::Recording => return Ok(None),
    };
    let dir = super::trickplay_dir(storage_root, file_id);
    let area = tokio::task::spawn_blocking(move || {
        read_trickplay(&dir)
            .ok()
            .and_then(|frames| detect(&frames, &exclude))
    })
    .await
    .ok()
    .flatten();
    file_pictures::record(
        pool,
        &PictureCheckFile {
            media_type,
            file_id,
        },
        area,
    )
    .await?;
    Ok(area)
}

/// Every thumbnail in a trickplay directory with the time it shows, cut from the sprite
/// sheets `thumbnails.vtt` maps them into.
pub fn read_trickplay(dir: &Path) -> std::io::Result<Vec<(f64, GrayImage)>> {
    let vtt = std::fs::read_to_string(dir.join("thumbnails.vtt"))?;
    let mut sheets: std::collections::HashMap<String, GrayImage> = std::collections::HashMap::new();
    let mut frames = Vec::new();
    let mut start = None;
    for line in vtt.lines() {
        if let Some((from, _)) = line.split_once(" --> ") {
            start = parse_timestamp(from);
            continue;
        }
        let (Some(at), Some((sheet, xywh))) = (start.take(), line.split_once("#xywh=")) else {
            continue;
        };
        let [x, y, w, h] = match xywh
            .split(',')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(v) if v.len() == 4 => [v[0], v[1], v[2], v[3]],
            _ => continue,
        };
        if !sheets.contains_key(sheet) {
            let Ok(img) = image::open(dir.join(sheet)) else {
                continue;
            };
            sheets.insert(sheet.to_string(), img.to_luma8());
        }
        let img = &sheets[sheet];
        if x + w <= img.width() && y + h <= img.height() {
            frames.push((at, image::imageops::crop_imm(img, x, y, w, h).to_image()));
        }
    }
    Ok(frames)
}

fn parse_timestamp(text: &str) -> Option<f64> {
    let mut secs = 0.0;
    for part in text.trim().split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 320;
    const H: u32 = 180;

    /// A frame whose picture spans `rows` and `cols`, black elsewhere, with a bright
    /// logo optionally stamped into the bottom bar.
    fn frame(
        rows: std::ops::Range<u32>,
        cols: std::ops::Range<u32>,
        luma: u8,
        logo_in_bar: bool,
    ) -> GrayImage {
        GrayImage::from_fn(W, H, |x, y| {
            let logo = logo_in_bar && (H - 14..H - 4).contains(&y) && (W - 50..W - 10).contains(&x);
            let picture = rows.contains(&y) && cols.contains(&x);
            image::Luma([if logo {
                235
            } else if picture {
                luma.saturating_add(((x + y) % 40) as u8)
            } else {
                3
            }])
        })
    }

    fn timed(frames: Vec<GrayImage>) -> Vec<(f64, GrayImage)> {
        frames
            .into_iter()
            .enumerate()
            .map(|(i, f)| (i as f64 * 5.0, f))
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn a_letterbox_is_found_even_with_a_logo_in_the_bar() {
        let frames = timed(
            (0..40)
                .map(|i| frame(23..157, 0..W, 90, i % 2 == 0))
                .collect(),
        );
        let area = detect(&frames, &[]).unwrap();
        assert!(
            close(area.y, 23.0 / H as f64) && close(area.height, 134.0 / H as f64),
            "{area:?}"
        );
        assert!(close(area.x, 0.0) && close(area.width, 1.0), "{area:?}");
    }

    #[test]
    fn a_pillarbox_is_found() {
        let frames = timed((0..40).map(|_| frame(0..H, 40..280, 90, false)).collect());
        let area = detect(&frames, &[]).unwrap();
        assert!(
            close(area.x, 40.0 / W as f64) && close(area.width, 240.0 / W as f64),
            "{area:?}"
        );
    }

    #[test]
    fn a_full_frame_has_no_bars_and_dark_scenes_do_not_invent_them() {
        let mut frames: Vec<_> = (0..30).map(|_| frame(0..H, 0..W, 90, false)).collect();
        frames.extend((0..30).map(|_| frame(40..140, 0..W, 2, false)));
        let area = detect(&timed(frames), &[]).unwrap();
        assert_eq!(area, PictureArea::FULL);
    }

    #[test]
    fn bars_that_come_and_go_are_not_cropped_away() {
        let frames = timed(
            (0..40)
                .map(|i| {
                    if i % 3 == 0 {
                        frame(0..H, 0..W, 90, false)
                    } else {
                        frame(23..157, 0..W, 90, false)
                    }
                })
                .collect(),
        );
        assert_eq!(detect(&frames, &[]).unwrap(), PictureArea::FULL);
    }

    #[test]
    fn intros_and_credits_are_left_out() {
        let mut frames: Vec<_> = (0..12).map(|_| frame(0..H, 0..W, 90, false)).collect();
        frames.extend((0..40).map(|_| frame(23..157, 0..W, 90, false)));
        let area = detect(&timed(frames), &[(0.0, 58.0)]).unwrap();
        assert!(close(area.y, 23.0 / H as f64), "{area:?}");
    }

    /// A comedian lit centre stage in a dark room: the room is dim, not black.
    #[test]
    fn a_dark_set_around_the_subject_is_not_a_bar() {
        let stage = |_| {
            GrayImage::from_fn(W, H, |x, y| {
                let lit = (60..260).contains(&x) && (35..150).contains(&y);
                image::Luma([if lit {
                    140
                } else {
                    8 + ((x * 7 + y * 3) % 6) as u8
                }])
            })
        };
        assert_eq!(
            detect(&timed((0..30).map(stage).collect()), &[]).unwrap(),
            PictureArea::FULL
        );
    }

    #[test]
    fn too_few_readable_frames_say_nothing() {
        assert_eq!(
            detect(
                &timed((0..5).map(|_| frame(23..157, 0..W, 90, false)).collect()),
                &[]
            ),
            None
        );
    }
}
