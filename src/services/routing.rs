use std::sync::Arc;

use tokio::sync::RwLock;

use crate::error::Result;
use crate::services::agent::AgentService;

pub struct RoutingService {
    agent_service: Arc<AgentService>,
    last_agent: RwLock<Option<String>>,
}

impl RoutingService {
    pub fn new(agent_service: Arc<AgentService>) -> Self {
        Self {
            agent_service,
            last_agent: RwLock::new(None),
        }
    }

    pub async fn route_query(&self, query: &str) -> Result<String> {
        let agents = self.agent_service.get_all_ai_agents();
        if agents.is_empty() {
            return Ok("default".to_string());
        }
        if agents.len() == 1 {
            let name = agents
                .keys()
                .next()
                .cloned()
                .unwrap_or("default".to_string());
            *self.last_agent.write().await = Some(name.clone());
            return Ok(name);
        }

        let trimmed = query.trim().to_lowercase();
        let short_replies = ["", "yes", "no", "ok", "k", "y", "n", "1", "0"];
        if short_replies.contains(&trimmed.as_str()) {
            if let Some(last) = self.last_agent.read().await.clone() {
                return Ok(last);
            }
        }

        let mut best = None;
        let mut best_score = 0usize;
        for (name, agent) in agents.iter() {
            let mut score = 0;
            if trimmed.contains(&name.to_lowercase()) {
                score += 10;
            }
            let spec = agent.specialization.to_lowercase();
            if !spec.is_empty() {
                for token in spec.split_whitespace() {
                    if trimmed.contains(token) {
                        score += 1;
                    }
                }
            }
            if score > best_score {
                best_score = score;
                best = Some(name.clone());
            }
        }

        let selected = best.unwrap_or_else(|| agents.keys().next().cloned().unwrap());
        *self.last_agent.write().await = Some(selected.clone());
        Ok(selected)
    }
}
