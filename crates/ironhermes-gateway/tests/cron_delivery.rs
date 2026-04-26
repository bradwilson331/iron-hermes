//! Phase 22.4.2.1 Plan 02 — regression test for cron -> Telegram delivery.
//!
//! Verifies that dispatch_delivery with a telegram DeliveryTarget invokes
//! TgSendApi::send_message via a FakeTgClient that captures call args.
//! No live HTTP — pure trait-object dispatch.

use std::sync::{Arc, Mutex};

use ironhermes_cron::DeliveryTarget;
use ironhermes_gateway::dispatch_delivery;
use ironhermes_gateway::telegram::TgSendApi;

struct FakeTgClient {
    pub calls: Arc<Mutex<Vec<(String, String, Option<String>)>>>, // (chat_id, content, thread_id)
    pub fail: bool,
}

#[async_trait::async_trait]
impl TgSendApi for FakeTgClient {
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.calls.lock().unwrap().push((
            chat_id.to_string(),
            content.to_string(),
            thread_id.map(|s| s.to_string()),
        ));
        if self.fail {
            Err(anyhow::anyhow!("fake send failure"))
        } else {
            Ok(())
        }
    }
}

fn fake_tg(fail: bool) -> Arc<FakeTgClient> {
    Arc::new(FakeTgClient {
        calls: Arc::new(Mutex::new(vec![])),
        fail,
    })
}

/// A telegram DeliveryTarget should invoke send_message with the correct chat_id + content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telegram_target_invokes_send_message() {
    let fake = fake_tg(false);
    let target = DeliveryTarget {
        platform: "telegram".to_string(),
        chat_id: "12345".to_string(),
        thread_id: None,
    };
    let tg_client: Option<Arc<dyn TgSendApi>> = Some(fake.clone());
    dispatch_delivery(Some(target), "hello world", &tg_client, "test-job").await;
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "expected exactly one send_message call");
    assert_eq!(calls[0].0, "12345", "chat_id must match target.chat_id");
    assert_eq!(calls[0].1, "hello world", "content must match job output");
    assert_eq!(calls[0].2, None, "thread_id must be None");
}

/// A local (None) DeliveryTarget must NOT invoke send_message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_target_does_not_call_tg() {
    let fake = fake_tg(false);
    let tg_client: Option<Arc<dyn TgSendApi>> = Some(fake.clone());
    dispatch_delivery(None, "some output", &tg_client, "test-job").await;
    let calls = fake.calls.lock().unwrap();
    assert!(calls.is_empty(), "local target must not invoke send_message");
}

/// A TG send failure must be non-fatal: dispatch_delivery returns normally, no panic,
/// and the attempted call is captured (D-09 non-fatal contract).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tg_send_failure_is_non_fatal() {
    let fake = fake_tg(true); // fail=true → send_message returns Err
    let target = DeliveryTarget {
        platform: "telegram".to_string(),
        chat_id: "99999".to_string(),
        thread_id: None,
    };
    let tg_client: Option<Arc<dyn TgSendApi>> = Some(fake.clone());
    // Must return without panic — D-09: delivery failure is non-fatal
    dispatch_delivery(Some(target), "error output", &tg_client, "test-job").await;
    // Call was attempted (captures before failure)
    let calls = fake.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "send_message was attempted even though it failed");
    assert_eq!(calls[0].0, "99999");
}
