//! Phase 22.4.2.1 Plan 02 — regression test for cron -> Telegram delivery.
//!
//! Verifies execute_cron_job with deliver=telegram:<id> invokes TgSendApi::send_message
//! via a FakeTgClient that captures call args. No live HTTP — pure trait-object dispatch.

use std::sync::{Arc, Mutex};

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

// Task 2 enables real dispatch assertions. Stubs keep the file compiling at Wave 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn telegram_target_invokes_send_message() {
    // TODO(Plan 02 Task 2): construct CronJob with deliver="telegram:12345",
    // call execute_cron_job(..., tg_client=Some(fake.clone())).
    // Assert fake.calls.lock().unwrap().len() == 1 and calls[0].0 == "12345".
    assert!(true, "wired in Task 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_target_does_not_call_tg() {
    assert!(true, "wired in Task 2");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tg_send_failure_is_non_fatal() {
    assert!(true, "wired in Task 2");
}
