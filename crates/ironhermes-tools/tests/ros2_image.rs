//! D-13 vision-path integration tests for camera-topic `subscribe` (Phase
//! 49.8 Plan 06 Task 2).
//!
//! `ros2_image_raw_to_vision` is this task's RED/GREEN target test — see
//! `49.8-06-SUMMARY.md`'s TDD Gate Compliance section for the recorded RED
//! evidence. `MockVision` is an in-test `VisionClientHandle` that records
//! every call it receives so tests can assert the tool never places the raw
//! frame (a `data:image` URI or base64 payload) anywhere in its own result —
//! only `MockVision`'s answer, describing what it "saw", ever appears there.

#[path = "support/mock_rosbridge.rs"]
mod mock_rosbridge;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine as _;
use ironhermes_core::Config;
use ironhermes_core::config::Ros2AllowEntry;
use ironhermes_tools::browser_vision::VisionClientHandle;
use ironhermes_tools::registry::ToolRegistry;
use ironhermes_tools::ros2::Ros2ConfigSource;
use mock_rosbridge::{MockConfig, MockRosbridge};
use serde_json::json;

fn fixed_config(bridge_url: Option<&str>, publish: Vec<Ros2AllowEntry>) -> Config {
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = bridge_url.map(|s| s.to_string());
    cfg.ros2.publish = publish;
    cfg
}

/// `(prompt, image_data_url)` pairs recorded by a [`MockVision`].
type RecordedCalls = Arc<Mutex<Vec<(String, String)>>>;

/// Records every `(prompt, image_data_url)` pair it is called with and
/// always answers `answer`.
struct MockVision {
    calls: RecordedCalls,
    answer: String,
}

#[async_trait]
impl VisionClientHandle for MockVision {
    async fn vision_call(&self, prompt: String, image_data_url: String) -> anyhow::Result<String> {
        self.calls
            .lock()
            .expect("calls mutex poisoned")
            .push((prompt, image_data_url));
        Ok(self.answer.clone())
    }
}

fn mock_vision(answer: &str) -> (Arc<MockVision>, RecordedCalls) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        Arc::new(MockVision {
            calls: calls.clone(),
            answer: answer.to_string(),
        }),
        calls,
    )
}

fn rgb8_image_msg(width: u32, height: u32, fill: [u8; 3]) -> serde_json::Value {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for _ in 0..(width * height) {
        data.extend_from_slice(&fill);
    }
    json!({
        "width": width,
        "height": height,
        "step": width * 3,
        "encoding": "rgb8",
        "data": base64::engine::general_purpose::STANDARD.encode(&data),
    })
}

fn small_jpeg_compressed_msg(width: u32, height: u32) -> serde_json::Value {
    let mut jpeg_bytes = Vec::new();
    let img = image::RgbImage::from_fn(width, height, |_, _| image::Rgb([30, 60, 90]));
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 90)
        .encode_image(&img)
        .expect("test-fixture JPEG encode must succeed");
    json!({
        "format": "jpeg",
        "data": base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes),
    })
}

// ---------------------------------------------------------------------------
// ros2_image_raw_to_vision (D-13, RED/GREEN target)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_image_raw_to_vision() {
    let image_msg = rgb8_image_msg(64, 48, [10, 20, 30]);
    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([(
            "/camera/image_raw".to_string(),
            "sensor_msgs/msg/Image".to_string(),
        )]),
        topic_messages: HashMap::from([("/camera/image_raw".to_string(), vec![image_msg])]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;

    let cfg = fixed_config(Some(&mock.url), vec![]);
    let (vision, calls) = mock_vision("a red ball on the floor");
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), Some(vision));

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/camera/image_raw",
                "type": "sensor_msgs/msg/Image",
                "max_messages": 5,
                "prompt": "is there a ball?"
            }),
        )
        .await
        .expect("subscribe dispatch must not error");

    assert!(result.contains("64x48"), "got: {result}");
    assert!(result.contains("rgb8"), "got: {result}");
    assert!(result.contains("a red ball on the floor"), "got: {result}");

    let recorded = calls.lock().expect("calls mutex poisoned");
    assert_eq!(
        recorded.len(),
        1,
        "vision must be called exactly once; got: {recorded:?}"
    );
    assert_eq!(recorded[0].0, "is there a ball?");
    assert!(
        recorded[0].1.starts_with("data:image/jpeg;base64,"),
        "got: {}",
        recorded[0].1
    );
}

// ---------------------------------------------------------------------------
// ros2_compressed_image_to_vision (D-13)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_compressed_image_to_vision() {
    let compressed_msg = small_jpeg_compressed_msg(32, 24);
    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([(
            "/camera/image_compressed".to_string(),
            "sensor_msgs/msg/CompressedImage".to_string(),
        )]),
        topic_messages: HashMap::from([(
            "/camera/image_compressed".to_string(),
            vec![compressed_msg],
        )]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;

    let cfg = fixed_config(Some(&mock.url), vec![]);
    let (vision, calls) = mock_vision("a hallway with a chair");
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), Some(vision));

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/camera/image_compressed",
                "type": "sensor_msgs/msg/CompressedImage",
                "max_messages": 5
            }),
        )
        .await
        .expect("subscribe dispatch must not error");

    assert!(result.contains("32x24"), "got: {result}");
    assert!(result.contains("a hallway with a chair"), "got: {result}");

    let recorded = calls.lock().expect("calls mutex poisoned");
    assert_eq!(
        recorded.len(),
        1,
        "vision must be called exactly once; got: {recorded:?}"
    );
    assert!(
        recorded[0].1.starts_with("data:image/jpeg;base64,"),
        "got: {}",
        recorded[0].1
    );
}

// ---------------------------------------------------------------------------
// ros2_image_output_never_contains_data_uri (D-13/T-49.8-16)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_image_output_never_contains_data_uri() {
    let raw_msg = rgb8_image_msg(64, 48, [5, 5, 5]);
    let compressed_msg = small_jpeg_compressed_msg(32, 24);

    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([
            (
                "/camera/image_raw".to_string(),
                "sensor_msgs/msg/Image".to_string(),
            ),
            (
                "/camera/image_compressed".to_string(),
                "sensor_msgs/msg/CompressedImage".to_string(),
            ),
        ]),
        topic_messages: HashMap::from([
            ("/camera/image_raw".to_string(), vec![raw_msg]),
            ("/camera/image_compressed".to_string(), vec![compressed_msg]),
        ]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;

    let cfg = fixed_config(Some(&mock.url), vec![]);
    let (vision, _calls) = mock_vision("nothing but a wall");
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), Some(vision));

    for (topic, ty) in [
        ("/camera/image_raw", "sensor_msgs/msg/Image"),
        (
            "/camera/image_compressed",
            "sensor_msgs/msg/CompressedImage",
        ),
    ] {
        let result = registry
            .dispatch(
                "ros2",
                json!({"action": "subscribe", "topic": topic, "type": ty, "max_messages": 5}),
            )
            .await
            .expect("subscribe dispatch must not error");
        assert!(
            !result.contains("data:image") && !result.contains("base64"),
            "the tool result for {topic} must never contain the raw frame; got: {result}"
        );
    }
}

// ---------------------------------------------------------------------------
// ros2_image_without_vision_handle_errors (D-13)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ros2_image_without_vision_handle_errors() {
    let image_msg = rgb8_image_msg(64, 48, [1, 2, 3]);
    let mock = MockRosbridge::start_with(MockConfig {
        topic_types: HashMap::from([(
            "/camera/image_raw".to_string(),
            "sensor_msgs/msg/Image".to_string(),
        )]),
        topic_messages: HashMap::from([("/camera/image_raw".to_string(), vec![image_msg])]),
        publish_interval_ms: 20,
        ..Default::default()
    })
    .await;

    let cfg = fixed_config(Some(&mock.url), vec![]);
    let mut registry = ToolRegistry::new();
    registry.register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);

    let result = registry
        .dispatch(
            "ros2",
            json!({
                "action": "subscribe",
                "topic": "/camera/image_raw",
                "type": "sensor_msgs/msg/Image",
                "max_messages": 5
            }),
        )
        .await
        .expect("subscribe dispatch must not error");

    assert!(result.starts_with("Received 1/1"), "got: {result}");
    assert!(
        result.contains("Error: no vision-capable model route"),
        "got: {result}"
    );
    assert!(result.contains("64x48"), "got: {result}");
}
