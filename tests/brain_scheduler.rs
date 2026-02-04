use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use butterfly_bot::brain::manager::BrainManager;
use butterfly_bot::interfaces::brain::{BrainContext, BrainEvent, BrainPlugin};
use butterfly_bot::interfaces::scheduler::ScheduledJob;
use butterfly_bot::scheduler::Scheduler;
use butterfly_bot::Result;

struct DummyBrain {
    name: String,
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl BrainPlugin for DummyBrain {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "dummy"
    }

    async fn on_event(&self, event: BrainEvent, _ctx: &BrainContext) -> Result<()> {
        let label = match event {
            BrainEvent::Start => "start",
            BrainEvent::Tick => "tick",
            BrainEvent::UserMessage { .. } => "user",
            BrainEvent::AssistantResponse { .. } => "assistant",
        };
        let mut guard = self.seen.lock().unwrap();
        guard.push(label.to_string());
        Ok(())
    }
}

struct TickJob {
    count: Arc<Mutex<u32>>,
}

#[async_trait]
impl ScheduledJob for TickJob {
    fn name(&self) -> &str {
        "tick"
    }

    fn interval(&self) -> Duration {
        Duration::from_millis(10)
    }

    async fn run(&self) -> Result<()> {
        let mut guard = self.count.lock().unwrap();
        *guard += 1;
        Ok(())
    }
}

#[tokio::test]
async fn brain_manager_loads_and_dispatches() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let config = json!({"brains": ["dummy"]});
    let mut manager = BrainManager::new(config);

    let seen_factory = seen.clone();
    manager.register_factory("dummy", move |_| {
        Arc::new(DummyBrain {
            name: "dummy".to_string(),
            seen: seen_factory.clone(),
        })
    });

    let loaded = manager.load_plugins();
    assert_eq!(loaded, vec!["dummy".to_string()]);

    let ctx = BrainContext {
        agent_name: "agent".to_string(),
        user_id: None,
    };
    manager.dispatch(BrainEvent::Start, &ctx).await;

    let guard = seen.lock().unwrap();
    assert_eq!(guard.as_slice(), ["start".to_string()]);
}

#[tokio::test]
async fn scheduler_runs_jobs() {
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        let count = Arc::new(Mutex::new(0u32));
        let mut scheduler = Scheduler::new();
        scheduler.register_job(Arc::new(TickJob {
            count: count.clone(),
        }));

        scheduler.start();
        tokio::time::sleep(Duration::from_millis(35)).await;
        scheduler.stop().await;

        let guard = count.lock().unwrap();
        *guard
    })
    .await;

    let count = result.expect("scheduler test timed out");
    assert!(count >= 2);
}
