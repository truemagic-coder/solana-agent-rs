use std::path::Path;
use std::sync::Arc;

use futures::stream::BoxStream;

use crate::config::Config;
use crate::error::{ButterflyBotError, Result};
use crate::factories::agent_factory::ButterflyBotFactory;
use crate::interfaces::plugins::Tool;
use crate::services::query::{ProcessOptions, ProcessResult, QueryService, UserInput};
use crate::services::agent::UiEvent;
use tokio::sync::broadcast;

pub struct ButterflyBot {
    query_service: QueryService,
}

impl ButterflyBot {
    pub async fn from_config(config: Config) -> Result<Self> {
        let query_service = ButterflyBotFactory::create_from_config(config).await?;
        Ok(Self { query_service })
    }

    pub async fn from_config_with_events(
        config: Config,
        ui_event_tx: Option<broadcast::Sender<UiEvent>>,
    ) -> Result<Self> {
        let query_service =
            ButterflyBotFactory::create_from_config_with_events(config, ui_event_tx).await?;
        Ok(Self { query_service })
    }

    pub async fn from_store(db_path: &str) -> Result<Self> {
        let config = Config::from_store(db_path)?.resolve_vault()?;
        let agent = Self::from_config(config).await?;
        Ok(agent)
    }

    pub async fn from_store_with_events(
        db_path: &str,
        ui_event_tx: Option<broadcast::Sender<UiEvent>>,
    ) -> Result<Self> {
        let config = Config::from_store(db_path)?.resolve_vault()?;
        let agent = Self::from_config_with_events(config, ui_event_tx).await?;
        Ok(agent)
    }

    pub async fn from_config_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let config = Config::from_file(path)?;
        let agent = Self::from_config(config).await?;
        Ok(agent)
    }

    pub fn process_text_stream<'a>(
        &'a self,
        user_id: &'a str,
        message: &'a str,
        prompt: Option<&'a str>,
    ) -> BoxStream<'a, Result<String>> {
        let service = &self.query_service;
        service.process_text_stream(user_id, message, prompt)
    }

    pub async fn process(
        &self,
        user_id: &str,
        input: UserInput,
        options: ProcessOptions,
    ) -> Result<ProcessResult> {
        self.query_service.process(user_id, input, options).await
    }

    pub async fn delete_user_history(&self, user_id: &str) -> Result<()> {
        self.query_service.delete_user_history(user_id).await
    }

    pub async fn get_user_history(&self, user_id: &str, limit: usize) -> Result<Vec<String>> {
        self.query_service.get_user_history(user_id, limit).await
    }

    pub async fn search_memory(
        &self,
        user_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        self.query_service
            .search_memory(user_id, query, limit)
            .await
    }

    pub async fn register_tool(&self, agent_name: &str, tool: Arc<dyn Tool>) -> Result<bool> {
        let agent_service = self.query_service.agent_service();
        let registry = agent_service.tool_registry.clone();
        if !registry.register_tool(tool.clone()).await {
            return Ok(false);
        }
        let assigned = registry.assign_tool_to_agent(agent_name, tool.name()).await;
        if !assigned {
            return Err(ButterflyBotError::Runtime(
                "Tool registered but could not assign to agent".to_string(),
            ));
        }
        Ok(true)
    }

    pub async fn brain_tick(&self) {
        let agent_service = self.query_service.agent_service();
        agent_service.dispatch_brain_tick().await;
    }
}
