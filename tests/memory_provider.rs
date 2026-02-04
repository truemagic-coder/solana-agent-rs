use serde_json::json;
use tokio::sync::Mutex;

use butterfly_bot::error::Result;
use butterfly_bot::interfaces::providers::MemoryProvider;
use butterfly_bot::providers::memory::InMemoryMemoryProvider;

struct DummyMemoryProvider {
    messages: Mutex<Vec<(String, String, String)>>,
}

impl DummyMemoryProvider {
    fn new() -> Self {
        Self {
            messages: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl MemoryProvider for DummyMemoryProvider {
    async fn append_message(&self, user_id: &str, role: &str, content: &str) -> Result<()> {
        self.messages.lock().await.push((
            user_id.to_string(),
            role.to_string(),
            content.to_string(),
        ));
        Ok(())
    }

    async fn get_history(&self, user_id: &str, _limit: usize) -> Result<Vec<String>> {
        let guard = self.messages.lock().await;
        Ok(guard
            .iter()
            .filter(|(u, _, _)| u == user_id)
            .map(|(_, role, content)| format!("{}: {}", role, content))
            .collect())
    }

    async fn clear_history(&self, user_id: &str) -> Result<()> {
        let mut guard = self.messages.lock().await;
        guard.retain(|(u, _, _)| u != user_id);
        Ok(())
    }
}

#[tokio::test]
async fn memory_provider_defaults_and_in_memory() {
    let provider = InMemoryMemoryProvider::new();
    provider.append_message("u1", "user", "hi").await.unwrap();
    provider
        .append_message("u1", "assistant", "hello")
        .await
        .unwrap();

    let history = provider.get_history("u1", 1).await.unwrap();
    assert_eq!(history.len(), 1);

    let all = provider.get_history("u1", 0).await.unwrap();
    assert_eq!(all.len(), 2);

    provider.clear_history("u1").await.unwrap();
    assert!(provider.get_history("u1", 0).await.unwrap().is_empty());

    provider
        .store(
            "u2",
            vec![
                json!({"role":"user","content":"a"}),
                json!({"role":"assistant","content":"b"}),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        provider
            .retrieve("u2")
            .await
            .unwrap()
            .lines()
            .collect::<Vec<_>>()
            .len(),
        2
    );
    let retrieved = provider.retrieve("u2").await.unwrap();
    let mut lines = retrieved.lines();
    assert!(lines.next().unwrap_or_default().ends_with("user: a"));
    assert!(lines.next().unwrap_or_default().ends_with("assistant: b"));
    provider.delete("u2").await.unwrap();

    let dummy = DummyMemoryProvider::new();
    dummy
        .store("u4", vec![json!({"role":"user","content":"x"})])
        .await
        .unwrap();
    assert_eq!(dummy.retrieve("u4").await.unwrap(), "user: x");
    dummy.delete("u4").await.unwrap();
    assert_eq!(
        dummy
            .find("any", json!(null), None, None, None)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(dummy.count_documents("any", json!(null)).unwrap(), 0);
}
