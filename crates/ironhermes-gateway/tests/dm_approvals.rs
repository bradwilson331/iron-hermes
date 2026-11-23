//! Phase 57 follow-up — Discord and Slack approval gate with DM-only routing.
//!
//! The approval prompt for a gated Warn goes to the REQUESTING USER'S DM and nowhere
//! else, and only an `/approve` or `/deny` sent by that same user from their DM resolves
//! it. These tests drive the real `GatewayMessageHandler` slash-command intercept and the
//! real `ApprovalCoordinator`; only the platform transport is a recording stub.
//!
//! Process isolation: `IRONHERMES_HOME` is process-global, so every test takes
//! [`ENV_LOCK`]. Run with `--test-threads=1`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ironhermes_core::{
    ApprovalGate, ApprovalOutcome, ApprovalsStore, AuditConfig, AuditLog, Config, MessageEvent,
    MessageResponse, Platform, ProviderResolver,
};
use ironhermes_gateway::adapter::{
    DirectMessageSender, DmOnlyAdapter, MessageHandler, PlatformAdapter,
};
use ironhermes_gateway::approval::{ApprovalCoordinator, GatewayApprovalGate};
use ironhermes_gateway::handler::GatewayMessageHandler;
use ironhermes_gateway::session::{SessionKey, SessionStore};
use ironhermes_tools::ToolRegistry;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const REQUESTER: &str = "111";
const OTHER_USER: &str = "222";

/// Records `(chat_id, content)` for every send.
struct Recorder {
    platform: Platform,
    sent: std::sync::Mutex<Vec<(String, String)>>,
}

impl Recorder {
    fn new(platform: Platform) -> Arc<Self> {
        Arc::new(Self {
            platform,
            sent: std::sync::Mutex::new(Vec::new()),
        })
    }
    fn sent(&self) -> Vec<(String, String)> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl PlatformAdapter for Recorder {
    fn platform(&self) -> Platform {
        self.platform.clone()
    }
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _t: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.sent
            .lock()
            .unwrap()
            .push((chat_id.to_string(), content.to_string()));
        Ok(MessageResponse {
            message_id: "m".into(),
            chat_id: chat_id.to_string(),
            platform: self.platform.clone(),
        })
    }
    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        t: Option<&str>,
    ) -> anyhow::Result<MessageResponse> {
        self.send_message(chat_id, content, t).await
    }
    async fn edit_message(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn edit_message_markdown_v2(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn delete_message(&self, _: &str, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn is_running(&self) -> bool {
        true
    }
}

/// Records `(user_id, content)` for every DM the approval coordinator sends.
struct DmRecorder {
    platform: Platform,
    sent: Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

#[async_trait]
impl DirectMessageSender for DmRecorder {
    fn platform(&self) -> Platform {
        self.platform.clone()
    }
    async fn send_direct_message(
        &self,
        user_id: &str,
        content: &str,
    ) -> anyhow::Result<MessageResponse> {
        self.sent
            .lock()
            .unwrap()
            .push((user_id.to_string(), content.to_string()));
        Ok(MessageResponse {
            message_id: "dm".into(),
            chat_id: format!("dm-channel-of-{user_id}"),
            platform: self.platform.clone(),
        })
    }
}

struct Rig {
    _home: tempfile::TempDir,
    _guard: tokio::sync::MutexGuard<'static, ()>,
    platform: Platform,
    handler: GatewayMessageHandler,
    store: Arc<RwLock<SessionStore>>,
    coord: Arc<ApprovalCoordinator>,
    /// The DM targets (user ids) and bodies the coordinator's adapter received.
    prompts: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// What the handler replied on the inbound surface.
    replies: Arc<Recorder>,
}

impl Rig {
    async fn new(platform: Platform, timeout_secs: u64) -> Self {
        let guard = ENV_LOCK.lock().await;
        let home = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_LOCK.
        unsafe { std::env::set_var("IRONHERMES_HOME", home.path()) };
        let config = Config::default();
        let state = Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(":memory:").unwrap(),
        ));
        let store = Arc::new(RwLock::new(SessionStore::new(state)));
        let mut handler = GatewayMessageHandler::new(
            config.clone(),
            ProviderResolver::build(&config).unwrap(),
            store.clone(),
            Arc::new(RwLock::new(ToolRegistry::new())),
        );
        let prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let adapter: Arc<dyn PlatformAdapter> = Arc::new(DmOnlyAdapter(DmRecorder {
            platform: platform.clone(),
            sent: prompts.clone(),
        }));
        let coord = Arc::new(ApprovalCoordinator::new(
            timeout_secs,
            adapter,
            Arc::new(ApprovalsStore::with_path(
                home.path().join("approvals.json"),
            )),
            Arc::new(AuditLog::with_path(
                home.path().join("audit.jsonl"),
                AuditConfig::default(),
            )),
        ));
        handler.set_approval_coordinator(coord.clone());
        Self {
            _home: home,
            _guard: guard,
            replies: Recorder::new(platform.clone()),
            platform,
            handler,
            store,
            coord,
            prompts,
        }
    }

    fn chat_ids(&self) -> (&'static str, &'static str) {
        match self.platform {
            Platform::Slack => ("C0CHANNEL", "D0DMCHAT"),
            _ => ("900", "800"),
        }
    }

    fn event(&self, chat_type: &str, chat: &str, sender: &str, content: &str) -> MessageEvent {
        MessageEvent {
            platform: self.platform.clone(),
            message_id: "m1".into(),
            chat_id: chat.to_string(),
            sender_id: sender.to_string(),
            content: content.to_string(),
            attachments: vec![],
            thread_id: None,
            chat_type: chat_type.to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    /// The canonical session id the agent loop would judge a turn from `chat`/`sender` under.
    async fn session_id(&self, chat: &str, sender: &str) -> String {
        let key = SessionKey::new(self.platform.clone(), chat).with_user(sender);
        let model = Config::default().model.default;
        self.store
            .write()
            .await
            .get_or_create(key, &model, &self.platform.to_string())
            .session_id
            .clone()
    }

    /// Start a gated request as the turn from `chat` by `REQUESTER` would, with the
    /// approval target the handler derives for that event. Waits for the prompt.
    async fn request_from(
        &self,
        chat_type: &str,
        chat: &str,
    ) -> tokio::task::JoinHandle<ApprovalOutcome> {
        let ev = self.event(chat_type, chat, REQUESTER, "do the thing");
        let target = ironhermes_gateway::handler::approval_target_for(&ev);
        let session_id = self.session_id(chat, REQUESTER).await;
        let gate = GatewayApprovalGate::new(self.coord.clone(), target);
        let before = self.prompts.lock().unwrap().len();
        let task = tokio::spawn(async move {
            gate.request_approval(
                &session_id,
                "terminal",
                "needs approval",
                &serde_json::json!({"command": "rm -rf build"}),
            )
            .await
        });
        for _ in 0..200 {
            if self.prompts.lock().unwrap().len() > before {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no approval prompt was sent");
    }

    async fn say(&self, chat_type: &str, chat: &str, sender: &str, content: &str) -> String {
        let before = self.replies.sent().len();
        let ev = self.event(chat_type, chat, sender, content);
        let adapter: Arc<dyn PlatformAdapter> = self.replies.clone();
        self.handler
            .handle(&ev, adapter, CancellationToken::new())
            .await
            .expect("handle");
        let sent = self.replies.sent();
        sent.get(before)
            .map(|(_, c)| c.clone())
            .expect("a reply to the command")
    }
}

fn platforms() -> [Platform; 2] {
    [Platform::Discord, Platform::Slack]
}

async fn finished(task: &mut tokio::task::JoinHandle<ApprovalOutcome>) -> Option<ApprovalOutcome> {
    tokio::time::timeout(Duration::from_millis(300), task)
        .await
        .ok()
        .map(|r| r.unwrap())
}

/// A DM-originated request is approved by the requester's `/approve` in that DM.
#[tokio::test]
async fn dm_approve_from_requester_resolves_dm_request() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 30).await;
        let (_, dm) = rig.chat_ids();
        let mut task = rig.request_from("dm", dm).await;
        let reply = rig.say("dm", dm, REQUESTER, "/approve").await;
        assert!(reply.contains("Approved"), "{p:?}: {reply}");
        assert_eq!(finished(&mut task).await, Some(ApprovalOutcome::Approved));
    }
}

/// A channel-originated request is approved by the requester's `/approve` from their DM
/// (the DM reply's session differs from the channel turn's session).
#[tokio::test]
async fn dm_approve_from_requester_resolves_channel_request() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 30).await;
        let (channel, dm) = rig.chat_ids();
        let mut task = rig.request_from("group", channel).await;
        let reply = rig.say("dm", dm, REQUESTER, "/approve").await;
        assert!(reply.contains("Approved"), "{p:?}: {reply}");
        assert_eq!(finished(&mut task).await, Some(ApprovalOutcome::Approved));
    }
}

/// `/deny` from the requester's DM denies.
#[tokio::test]
async fn dm_deny_from_requester_denies_channel_request() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 30).await;
        let (channel, dm) = rig.chat_ids();
        let mut task = rig.request_from("group", channel).await;
        let reply = rig.say("dm", dm, REQUESTER, "/deny").await;
        assert!(reply.contains("Denied"), "{p:?}: {reply}");
        assert_eq!(finished(&mut task).await, Some(ApprovalOutcome::Denied));
    }
}

/// `/approve` or `/deny` typed in a channel never resolves, even from the requester.
#[tokio::test]
async fn approve_in_channel_does_not_resolve() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 30).await;
        let (channel, _) = rig.chat_ids();
        let mut task = rig.request_from("group", channel).await;
        for cmd in ["/approve", "/deny"] {
            let reply = rig.say("group", channel, REQUESTER, cmd).await;
            assert!(
                reply.to_lowercase().contains("dm"),
                "{p:?}: a channel {cmd} is told approvals are answered in DM: {reply}"
            );
            assert!(
                !reply.contains("Approved") && !reply.contains("Denied"),
                "{p:?}: {reply}"
            );
        }
        assert_eq!(finished(&mut task).await, None, "{p:?}: still pending");
    }
}

/// Another user (whitelisted or not) cannot approve someone else's request, from a DM or
/// from the channel.
#[tokio::test]
async fn other_user_cannot_resolve() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 30).await;
        let (channel, dm) = rig.chat_ids();
        let mut task = rig.request_from("group", channel).await;
        let from_dm = rig.say("dm", dm, OTHER_USER, "/approve").await;
        assert!(from_dm.contains("No pending"), "{p:?}: {from_dm}");
        let from_channel = rig.say("group", channel, OTHER_USER, "/approve").await;
        assert!(!from_channel.contains("Approved"), "{p:?}: {from_channel}");
        assert_eq!(finished(&mut task).await, None, "{p:?}: still pending");
    }
}

/// Nobody answers: the coordinator timeout denies (fails closed).
#[tokio::test]
async fn timeout_denies() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 1).await;
        let (channel, _) = rig.chat_ids();
        let task = rig.request_from("group", channel).await;
        assert_eq!(task.await.unwrap(), ApprovalOutcome::TimedOut);
    }
}

/// A channel-originated gated Warn sends the prompt to the requester's DM target and
/// NEVER to the channel id; the prompt is not copied anywhere else.
#[tokio::test]
async fn channel_request_prompts_requesters_dm_not_the_channel() {
    for p in platforms() {
        let rig = Rig::new(p.clone(), 30).await;
        let (channel, _) = rig.chat_ids();
        let _task = rig.request_from("group", channel).await;
        let sent = rig.prompts.lock().unwrap().clone();
        assert!(
            sent.iter()
                .any(|(to, body)| to == REQUESTER && body.contains("Approval required")),
            "{p:?}: the prompt goes to the requester's DM target: {sent:?}"
        );
        assert!(
            !sent.iter().any(|(to, _)| to == channel),
            "{p:?}: the prompt must never target the channel id: {sent:?}"
        );
        assert!(
            rig.replies.sent().is_empty(),
            "{p:?}: nothing is posted on the inbound (channel) surface: {:?}",
            rig.replies.sent()
        );
    }
}
