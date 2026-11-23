//! ROS camera frame -> guarded, downscaled JPEG data URL (Phase 49.8 D-13,
//! plan 49.8-06).
//!
//! [`frame_to_jpeg_data_url`] converts a `sensor_msgs/Image` or
//! `sensor_msgs/CompressedImage` message into a `data:image/jpeg;base64,...`
//! data URL no wider than [`VISION_MAX_WIDTH`] px — every dimension, step, and
//! source-length guard runs BEFORE any pixel buffer is allocated (T-49.8-09).
//! [`describe_frame`] is the D-13 vision seam: it converts the frame, then
//! hands the data URL to the already-wired [`VisionClientHandle`] (the same
//! trait `browser_vision.rs` uses) and returns the model's TEXT description —
//! the data URL itself never becomes part of any tool result or log line
//! (49.8-03's spike proved a raw data URI in a tool result reaches every
//! provider as inert text, never an image the model can see). This is the
//! public seam the rospider `camera_snapshot` verb (49.8-09) reuses, so both
//! tools share one image path.

use std::io::Cursor;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::Value;
use tokio::time::Duration;
use tracing::debug;

use crate::browser_vision::VisionClientHandle;
use crate::ros2::canonical_type;

// ---------------------------------------------------------------------------
// Guards and tunables
// ---------------------------------------------------------------------------

/// D-13/T-49.8-09: a frame's width or height of 0 or above this value is
/// rejected before any pixel buffer is built.
pub const MAX_FRAME_DIMENSION: u32 = 4096;
/// T-49.8-09: the decoded/raw source payload (the `data` field, BEFORE any
/// base64 decode) may never exceed this many bytes — checked against the
/// base64 string's own length (or the JSON array's element count) before any
/// decode/allocation happens.
pub const MAX_IMAGE_SOURCE_BYTES: usize = 16 * 1024 * 1024;
/// D-13: no data URL this module produces is ever wider than this.
pub const VISION_MAX_WIDTH: u32 = 640;
/// JPEG re-encode quality for a downscaled/converted frame.
pub const JPEG_QUALITY: u8 = 80;
/// Wall-clock budget for the internal vision-model call.
pub const VISION_TIMEOUT_SECS: u64 = 25;
/// Default prompt when a camera-topic `subscribe` call supplies none.
pub const DEFAULT_CAMERA_PROMPT: &str = "Describe what the robot's camera sees: objects, \
    people, and obstacles, with their rough position (left/centre/right, near/far), and any \
    visible text.";

/// Dimensions/encoding of a converted frame — never carries the data URL or
/// raw bytes itself (D-13: the frame payload never leaves this module as
/// anything but a JPEG data URL, and never appears in a log line).
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub encoding: String,
    pub source_bytes: usize,
}

// ---------------------------------------------------------------------------
// Byte extraction
// ---------------------------------------------------------------------------

/// `data` may be a base64 string (rosbridge's default `uint8[]` encoding) or
/// a JSON array of integers. Rejects when the decoded length would exceed
/// [`MAX_IMAGE_SOURCE_BYTES`] — for the base64 case this is checked against
/// the ENCODED string length BEFORE any decode/allocation happens (base64
/// expands ~4:3, so the encoded length is always >= the decoded length).
fn extract_bytes(data: &Value) -> Result<Vec<u8>, String> {
    match data {
        Value::String(s) => {
            let approx_decoded = (s.len() as u64) * 3 / 4;
            if approx_decoded > MAX_IMAGE_SOURCE_BYTES as u64 {
                return Err(format!(
                    "frame data exceeds the {MAX_IMAGE_SOURCE_BYTES}-byte source limit"
                ));
            }
            base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map_err(|e| format!("frame data is not valid base64: {e}"))
        }
        Value::Array(arr) => {
            if arr.len() > MAX_IMAGE_SOURCE_BYTES {
                return Err(format!(
                    "frame data exceeds the {MAX_IMAGE_SOURCE_BYTES}-byte source limit"
                ));
            }
            let mut bytes = Vec::with_capacity(arr.len());
            for v in arr {
                let byte = v
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .or_else(|| v.as_i64().and_then(|n| u8::try_from(n).ok()))
                    .ok_or_else(|| {
                        "frame data array contains a value outside the 0..=255 byte range"
                            .to_string()
                    })?;
                bytes.push(byte);
            }
            Ok(bytes)
        }
        _ => Err("frame data field is missing or has an unsupported JSON type".to_string()),
    }
}

// ---------------------------------------------------------------------------
// sensor_msgs/CompressedImage
// ---------------------------------------------------------------------------

fn compressed_image_to_jpeg(msg: &Value) -> Result<(String, FrameInfo), String> {
    let format = msg.get("format").and_then(Value::as_str).unwrap_or("");
    let format_lower = format.to_ascii_lowercase();
    let mime = if format_lower.contains("jpeg") || format_lower.contains("jpg") {
        "image/jpeg"
    } else if format_lower.contains("png") {
        "image/png"
    } else {
        return Err(format!("unsupported CompressedImage format '{format}'"));
    };

    let Some(data) = msg.get("data") else {
        return Err("CompressedImage message has no 'data' field".to_string());
    };
    let bytes = extract_bytes(data)?;
    if bytes.is_empty() {
        return Err("CompressedImage data is empty".to_string());
    }

    // Dimensions are read from the header only (no full pixel decode) —
    // T-49.8-09's "before any allocation" guard applies here too.
    let (width, height) = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| format!("cannot read CompressedImage bytes: {e}"))?
        .into_dimensions()
        .map_err(|e| format!("cannot read CompressedImage dimensions: {e}"))?;
    if width == 0 || height == 0 || width > MAX_FRAME_DIMENSION || height > MAX_FRAME_DIMENSION {
        return Err(format!(
            "CompressedImage dimensions {width}x{height} are outside the 1..={MAX_FRAME_DIMENSION} range"
        ));
    }

    let info = FrameInfo {
        width,
        height,
        encoding: format.to_string(),
        source_bytes: bytes.len(),
    };

    // No re-encode needed: already a small-enough JPEG — pass the original
    // bytes through untouched (D-13's "no re-encode when it already fits").
    if width <= VISION_MAX_WIDTH && mime == "image/jpeg" {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        return Ok((format!("data:image/jpeg;base64,{b64}"), info));
    }

    // Either too wide, or a non-JPEG format (PNG) — decode fully, downscale
    // if needed, and re-encode as JPEG.
    let decoded = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| format!("cannot read CompressedImage bytes: {e}"))?
        .decode()
        .map_err(|e| format!("cannot decode CompressedImage: {e}"))?;
    let data_url = downscale_and_encode_jpeg(&decoded.to_rgb8())?;
    Ok((data_url, info))
}

// ---------------------------------------------------------------------------
// sensor_msgs/Image
// ---------------------------------------------------------------------------

/// Bytes-per-pixel used for the `step >= width * channels` guard. YUYV is 2
/// bytes/pixel on average (4 bytes cover a 2-pixel group).
fn channels_for_encoding(encoding: &str) -> Result<u64, String> {
    match encoding {
        "rgb8" | "bgr8" => Ok(3),
        "rgba8" | "bgra8" => Ok(4),
        "mono8" => Ok(1),
        "yuv422_yuy2" | "yuyv" => Ok(2),
        other => Err(format!("unsupported Image encoding '{other}'")),
    }
}

fn raw_image_to_jpeg(msg: &Value) -> Result<(String, FrameInfo), String> {
    let width_raw = msg.get("width").and_then(Value::as_u64).unwrap_or(0);
    let height_raw = msg.get("height").and_then(Value::as_u64).unwrap_or(0);
    if width_raw == 0
        || height_raw == 0
        || width_raw > MAX_FRAME_DIMENSION as u64
        || height_raw > MAX_FRAME_DIMENSION as u64
    {
        return Err(format!(
            "Image dimensions {width_raw}x{height_raw} are outside the 1..={MAX_FRAME_DIMENSION} range"
        ));
    }
    let width = width_raw as u32;
    let height = height_raw as u32;

    let encoding = msg
        .get("encoding")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let channels = channels_for_encoding(&encoding)?;

    let step = msg.get("step").and_then(Value::as_u64).unwrap_or(0);
    let min_step = width as u64 * channels;
    if step < min_step {
        return Err(format!(
            "Image step {step} is smaller than width * channels ({min_step})"
        ));
    }

    let Some(data) = msg.get("data") else {
        return Err("Image message has no 'data' field".to_string());
    };
    let bytes = extract_bytes(data)?;
    let required_len = step * height as u64;
    if (bytes.len() as u64) < required_len {
        return Err(format!(
            "Image data length {} is shorter than step * height ({required_len})",
            bytes.len()
        ));
    }

    let source_bytes = bytes.len();

    let mut img = image::RgbImage::new(width, height);
    match encoding.as_str() {
        "rgb8" => fill_rgb(&mut img, &bytes, width, height, step, 3, |b| {
            [b[0], b[1], b[2]]
        }),
        "bgr8" => fill_rgb(&mut img, &bytes, width, height, step, 3, |b| {
            [b[2], b[1], b[0]]
        }),
        "rgba8" => fill_rgb(&mut img, &bytes, width, height, step, 4, |b| {
            [b[0], b[1], b[2]]
        }),
        "bgra8" => fill_rgb(&mut img, &bytes, width, height, step, 4, |b| {
            [b[2], b[1], b[0]]
        }),
        "mono8" => fill_rgb(&mut img, &bytes, width, height, step, 1, |b| {
            [b[0], b[0], b[0]]
        }),
        "yuv422_yuy2" | "yuyv" => fill_rgb_yuyv(&mut img, &bytes, width, height, step),
        // Unreachable — channels_for_encoding already rejected anything else.
        _ => unreachable!("channels_for_encoding validates the encoding above"),
    }

    let info = FrameInfo {
        width,
        height,
        encoding,
        source_bytes,
    };
    let data_url = downscale_and_encode_jpeg(&img)?;
    Ok((data_url, info))
}

/// Fills `img` from `bytes` honoring `step` row padding (`step` may exceed
/// `width * channels`) — one pixel-group -> RGB `pixel_fn` per pixel.
fn fill_rgb(
    img: &mut image::RgbImage,
    bytes: &[u8],
    width: u32,
    height: u32,
    step: u64,
    channels: u64,
    pixel_fn: impl Fn(&[u8]) -> [u8; 3],
) {
    for y in 0..height {
        let row_start = (y as u64 * step) as usize;
        for x in 0..width {
            let px_start = row_start + (x as u64 * channels) as usize;
            let group = &bytes[px_start..px_start + channels as usize];
            img.put_pixel(x, y, image::Rgb(pixel_fn(group)));
        }
    }
}

/// YUYV 4:2:2 -> RGB, BT.601 integer math. Each 4-byte group (`Y0 U Y1 V`)
/// covers 2 horizontal pixels; a trailing odd pixel (if `width` is odd)
/// reuses the group's own luma without reading past the buffer.
fn fill_rgb_yuyv(img: &mut image::RgbImage, bytes: &[u8], width: u32, height: u32, step: u64) {
    for y in 0..height {
        let row_start = (y as u64 * step) as usize;
        let mut x = 0u32;
        while x < width {
            let group_start = row_start + (x as u64 * 2) as usize;
            let y0 = *bytes.get(group_start).unwrap_or(&0) as i32;
            let u = *bytes.get(group_start + 1).unwrap_or(&128) as i32;
            let y1 = bytes.get(group_start + 2).copied().unwrap_or(y0 as u8) as i32;
            let v = *bytes.get(group_start + 3).unwrap_or(&128) as i32;

            img.put_pixel(x, y, image::Rgb(yuv_to_rgb(y0, u, v)));
            if x + 1 < width {
                img.put_pixel(x + 1, y, image::Rgb(yuv_to_rgb(y1, u, v)));
            }
            x += 2;
        }
    }
}

/// BT.601 integer YUV -> RGB conversion (ITU-R BT.601 full-range coefficients).
fn yuv_to_rgb(y: i32, u: i32, v: i32) -> [u8; 3] {
    let c = y - 16;
    let d = u - 128;
    let e = v - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    [clamp_u8(r), clamp_u8(g), clamp_u8(b)]
}

fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

// ---------------------------------------------------------------------------
// Shared downscale + encode
// ---------------------------------------------------------------------------

/// Downscales `img` to at most [`VISION_MAX_WIDTH`] px wide (preserving
/// aspect ratio, `FilterType::Triangle`) when wider, then encodes as JPEG at
/// [`JPEG_QUALITY`] and returns the `data:image/jpeg;base64,...` data URL.
fn downscale_and_encode_jpeg(img: &image::RgbImage) -> Result<String, String> {
    let resized;
    let final_img: &image::RgbImage = if img.width() > VISION_MAX_WIDTH {
        let new_height = ((img.height() as f64) * (VISION_MAX_WIDTH as f64) / (img.width() as f64))
            .round()
            .max(1.0) as u32;
        resized = image::imageops::resize(
            img,
            VISION_MAX_WIDTH,
            new_height,
            image::imageops::FilterType::Triangle,
        );
        &resized
    } else {
        img
    };

    let mut buf: Vec<u8> = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY)
        .encode_image(final_img)
        .map_err(|e| format!("cannot encode frame as JPEG: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Converts a `sensor_msgs/Image` or `sensor_msgs/CompressedImage` message
/// (identified by `msg_type`, which may carry a `pkg/msg/Type` or `pkg/Type`
/// form — see [`canonical_type`]) into a JPEG data URL. Every guard
/// (dimension, step, source-length, unsupported encoding/format) runs before
/// any pixel buffer is allocated.
pub fn frame_to_jpeg_data_url(msg_type: &str, msg: &Value) -> Result<(String, FrameInfo), String> {
    match canonical_type(msg_type).as_str() {
        "sensor_msgs/CompressedImage" => compressed_image_to_jpeg(msg),
        "sensor_msgs/Image" => raw_image_to_jpeg(msg),
        other => Err(format!(
            "'{other}' is not a supported camera frame message type"
        )),
    }
}

/// D-13: converts `msg` and hands the resulting data URL to `vision`'s
/// internal vision-model call — the data URL is NEVER returned as part of
/// this function's own output or logged (only size/encoding are, at
/// `debug!`). Mirrors `browser_vision.rs`'s pattern exactly: the tool result
/// is always text, the frame itself never reaches the agent's context.
pub async fn describe_frame(
    msg_type: &str,
    msg: &Value,
    topic: &str,
    prompt: Option<&str>,
    vision: Option<&Arc<dyn VisionClientHandle>>,
) -> String {
    let (data_url, info) = match frame_to_jpeg_data_url(msg_type, msg) {
        Ok(v) => v,
        Err(e) => return format!("Error: cannot convert the frame from {topic}: {e}"),
    };
    debug!(
        width = info.width,
        height = info.height,
        encoding = %info.encoding,
        source_bytes = info.source_bytes,
        "ros2: converted camera frame for vision"
    );

    let Some(vision) = vision else {
        return format!(
            "Error: no vision-capable model route is wired on this surface — received a \
             {}x{} ({}) frame from {topic} but cannot describe it.",
            info.width, info.height, info.encoding
        );
    };

    let prompt = prompt.unwrap_or(DEFAULT_CAMERA_PROMPT).to_string();
    match tokio::time::timeout(
        Duration::from_secs(VISION_TIMEOUT_SECS),
        vision.vision_call(prompt, data_url),
    )
    .await
    {
        Ok(Ok(description)) => format!(
            "Camera frame {}x{} ({}) from {topic}:\n{description}",
            info.width, info.height, info.encoding
        ),
        Ok(Err(e)) => format!("Error: vision call failed for the frame from {topic}: {e}"),
        Err(_) => format!(
            "Error: vision call timed out after {VISION_TIMEOUT_SECS}s for the frame from {topic}"
        ),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rgb8_image_msg(width: u32, height: u32, pixels: &[[u8; 3]]) -> Value {
        let mut data = Vec::with_capacity(pixels.len() * 3);
        for p in pixels {
            data.extend_from_slice(p);
        }
        json!({
            "width": width,
            "height": height,
            "step": width * 3,
            "encoding": "rgb8",
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        })
    }

    #[test]
    fn ros2_image_rgb8_to_jpeg() {
        let pixels: Vec<[u8; 3]> = (0..8).map(|i| [i as u8 * 10, 50, 200]).collect();
        let msg = rgb8_image_msg(4, 2, &pixels);

        let (url, info) = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg)
            .expect("rgb8 4x2 frame must convert");
        assert!(url.starts_with("data:image/jpeg;base64,"), "got: {url}");
        assert_eq!(info.width, 4);
        assert_eq!(info.height, 2);
        assert_eq!(info.encoding, "rgb8");

        let payload = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let jpeg_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let decoded = image::load_from_memory(&jpeg_bytes).expect("must decode as an image");
        assert_eq!((decoded.width(), decoded.height()), (4, 2));
    }

    #[test]
    fn ros2_image_data_as_int_array() {
        let pixels: Vec<[u8; 3]> = (0..8).map(|i| [i as u8 * 10, 50, 200]).collect();
        let mut flat: Vec<u8> = Vec::new();
        for p in &pixels {
            flat.extend_from_slice(p);
        }
        let msg_b64 = rgb8_image_msg(4, 2, &pixels);
        let mut msg_array = msg_b64.clone();
        msg_array["data"] = json!(flat.iter().map(|b| json!(*b as i64)).collect::<Vec<_>>());

        let (url_b64, info_b64) = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg_b64)
            .expect("base64 form must convert");
        let (url_array, info_array) = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg_array)
            .expect("int-array form must convert");

        assert_eq!(
            url_b64, url_array,
            "base64 and int-array data must convert identically"
        );
        assert_eq!(info_b64.width, info_array.width);
        assert_eq!(info_b64.height, info_array.height);
    }

    #[test]
    fn ros2_image_bgr8_channel_order() {
        // 8x8 solid "pure red" patch in BGR order: B=0, G=0, R=255.
        let pixels: Vec<[u8; 3]> = (0..64).map(|_| [0, 0, 255]).collect();
        let mut data = Vec::with_capacity(pixels.len() * 3);
        for p in &pixels {
            data.extend_from_slice(p);
        }
        let msg = json!({
            "width": 8,
            "height": 8,
            "step": 8 * 3,
            "encoding": "bgr8",
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        });

        let (url, _info) =
            frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg).expect("bgr8 frame must convert");
        let payload = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let jpeg_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let decoded = image::load_from_memory(&jpeg_bytes).unwrap().to_rgb8();
        let px = decoded.get_pixel(4, 4);
        assert!(
            px[0] > 150 && px[0] > px[1] + 50 && px[0] > px[2] + 50,
            "expected a dominant red channel after bgr8->rgb conversion; got {px:?}"
        );
    }

    #[test]
    fn ros2_image_mono8() {
        let data: Vec<u8> = (0..8u8).map(|i| i * 20).collect();
        let msg = json!({
            "width": 4,
            "height": 2,
            "step": 4,
            "encoding": "mono8",
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        });

        let (url, info) = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg)
            .expect("mono8 frame must convert");
        let payload = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let jpeg_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let decoded = image::load_from_memory(&jpeg_bytes).expect("must decode");
        assert_eq!((decoded.width(), decoded.height()), (4, 2));
        assert_eq!(info.encoding, "mono8");
    }

    #[test]
    fn ros2_image_yuyv_to_rgb() {
        // 4x2 YUYV 4:2:2: 2 bytes/pixel average, 2 groups of 4 bytes per row.
        let row: Vec<u8> = vec![128, 128, 128, 128, 128, 128, 128, 128];
        let mut data = Vec::new();
        data.extend_from_slice(&row);
        data.extend_from_slice(&row);
        let msg = json!({
            "width": 4,
            "height": 2,
            "step": 8,
            "encoding": "yuv422_yuy2",
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        });

        let (url, info) =
            frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg).expect("yuyv frame must convert");
        let payload = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let jpeg_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let decoded = image::load_from_memory(&jpeg_bytes).expect("must decode");
        assert_eq!((decoded.width(), decoded.height()), (4, 2));
        assert_eq!(info.encoding, "yuv422_yuy2");
    }

    #[test]
    fn ros2_image_rejects_oversized_dimensions() {
        let msg = json!({
            "width": 100_000,
            "height": 100,
            "step": 300_000,
            "encoding": "rgb8",
        });
        let err = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg)
            .expect_err("oversized dimensions must be rejected");
        assert!(err.contains("4096"), "got: {err}");
    }

    #[test]
    fn ros2_image_rejects_short_data() {
        let short_data: Vec<u8> = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]; // step*height = 24
        let msg = json!({
            "width": 4,
            "height": 2,
            "step": 12,
            "encoding": "rgb8",
            "data": base64::engine::general_purpose::STANDARD.encode(&short_data),
        });
        let err = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg)
            .expect_err("short data must be rejected");
        assert!(err.contains("shorter than"), "got: {err}");
    }

    #[test]
    fn ros2_image_rejects_unsupported_encoding() {
        let msg = json!({
            "width": 4,
            "height": 2,
            "step": 16,
            "encoding": "32FC1",
            "data": base64::engine::general_purpose::STANDARD.encode(vec![0u8; 32]),
        });
        let err = frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg)
            .expect_err("unsupported encoding must be rejected");
        assert!(err.contains("32FC1"), "got: {err}");
    }

    #[test]
    fn ros2_compressed_jpeg_passthrough() {
        let mut small_jpeg = Vec::new();
        {
            let img = image::RgbImage::from_fn(8, 8, |_, _| image::Rgb([10, 20, 30]));
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut small_jpeg, 90)
                .encode_image(&img)
                .expect("test-fixture JPEG encode must succeed");
        }
        let msg = json!({
            "format": "jpeg",
            "data": base64::engine::general_purpose::STANDARD.encode(&small_jpeg),
        });

        let (url, info) = frame_to_jpeg_data_url("sensor_msgs/msg/CompressedImage", &msg)
            .expect("small in-width jpeg must pass through");
        assert_eq!(info.width, 8);
        assert_eq!(info.height, 8);
        let payload = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let payload_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        assert_eq!(
            payload_bytes, small_jpeg,
            "a small in-width JPEG must pass through unmodified"
        );
    }

    #[test]
    fn ros2_compressed_unsupported_format() {
        let msg = json!({
            "format": "tiff",
            "data": base64::engine::general_purpose::STANDARD.encode(vec![0u8; 16]),
        });
        let err = frame_to_jpeg_data_url("sensor_msgs/msg/CompressedImage", &msg)
            .expect_err("unsupported format must be rejected");
        assert!(err.contains("tiff"), "got: {err}");
    }

    #[test]
    fn ros2_image_downscales_wide_frames() {
        let width = 1280u32;
        let height = 720u32;
        let data = vec![100u8; (width * height * 3) as usize];
        let msg = json!({
            "width": width,
            "height": height,
            "step": width * 3,
            "encoding": "rgb8",
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        });

        let (url, info) =
            frame_to_jpeg_data_url("sensor_msgs/msg/Image", &msg).expect("wide frame must convert");
        assert_eq!(info.width, 1280);
        assert_eq!(info.height, 720);

        let payload = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let jpeg_bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap();
        let decoded = image::load_from_memory(&jpeg_bytes).expect("must decode");
        assert_eq!((decoded.width(), decoded.height()), (640, 360));
    }
}
