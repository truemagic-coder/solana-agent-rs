use std::sync::Arc;

use crate::config::Config;
use crate::domains::agent::BusinessMission;
use crate::error::{Result, SolanaAgentError};
use crate::guardrails::pii::{NoopGuardrail, PiiGuardrail};
use crate::interfaces::guardrails::{InputGuardrail, OutputGuardrail};
use crate::providers::memory::InMemoryMemoryProvider;
#[cfg(feature = "mongo")]
use crate::providers::mongodb::MongoMemoryProvider;
use crate::providers::openai::OpenAiProvider;
use crate::services::agent::AgentService;
use crate::services::query::QueryService;
use crate::services::routing::RoutingService;

pub struct SolanaAgentFactory;

impl SolanaAgentFactory {
    pub async fn create_from_config(config: Config) -> Result<QueryService> {
        let (api_key, model, base_url) = if let Some(openai) = config.openai {
            (openai.api_key, openai.model, openai.base_url)
        } else if let Some(groq) = config.groq {
            let base_url = groq
                .base_url
                .or_else(|| Some("https://api.groq.com/openai/v1".to_string()));
            (groq.api_key, groq.model, base_url)
        } else {
            return Err(SolanaAgentError::Config(
                "Missing openai or groq configuration".to_string(),
            ));
        };

        let llm = Arc::new(OpenAiProvider::new(api_key, model, base_url));

        let business_mission = config.business.map(|b| {
            let mut mission = BusinessMission::default();
            mission.mission = b.mission;
            mission.voice = b.voice;
            if let Some(values) = b.values {
                mission.values = values
                    .into_iter()
                    .map(|v| (v.name, v.description))
                    .collect();
            }
            mission.goals = b.goals.unwrap_or_default();
            mission
        });

        let mut input_guardrails: Vec<Arc<dyn InputGuardrail>> = Vec::new();
        let mut output_guardrails: Vec<Arc<dyn OutputGuardrail>> = Vec::new();

        if let Some(guardrails) = config.guardrails {
            if let Some(input) = guardrails.input {
                for spec in input {
                    match spec.class.as_str() {
                        "solana_agent.guardrails.pii.PII" | "PII" => {
                            input_guardrails.push(Arc::new(PiiGuardrail::new(spec.config)));
                        }
                        _ => {
                            input_guardrails.push(Arc::new(NoopGuardrail));
                        }
                    }
                }
            }
            if let Some(output) = guardrails.output {
                for spec in output {
                    match spec.class.as_str() {
                        "solana_agent.guardrails.pii.PII" | "PII" => {
                            output_guardrails.push(Arc::new(PiiGuardrail::new(spec.config)));
                        }
                        _ => {
                            output_guardrails.push(Arc::new(NoopGuardrail));
                        }
                    }
                }
            }
        }

        let mut agent_service = AgentService::new(llm, business_mission, output_guardrails);
        for agent in config.agents {
            agent_service.register_ai_agent(
                agent.name,
                agent.instructions,
                agent.specialization,
                agent.capture_name,
                agent.capture_schema,
            );
        }

        let agent_service = Arc::new(agent_service);
        let routing_service = Arc::new(RoutingService::new(agent_service.clone()));
        let memory_provider: Arc<dyn crate::interfaces::providers::MemoryProvider> =
            if let Some(_mongo) = config.mongo {
                #[cfg(feature = "mongo")]
                {
                    let collection = _mongo.collection.unwrap_or_else(|| "messages".to_string());
                    Arc::new(
                        MongoMemoryProvider::new(
                            &_mongo.connection_string,
                            &_mongo.database,
                            &collection,
                        )
                        .await?,
                    )
                }
                #[cfg(not(feature = "mongo"))]
                {
                    return Err(SolanaAgentError::Config(
                        "MongoDB support is disabled. Enable the 'mongo' feature.".to_string(),
                    ));
                }
            } else {
                Arc::new(InMemoryMemoryProvider::new())
            };

        Ok(QueryService::new(
            agent_service,
            routing_service,
            Some(memory_provider),
            input_guardrails,
        ))
    }
}
