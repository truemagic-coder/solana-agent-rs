use std::collections::HashSet;
use std::env;
use std::sync::Arc;

use serde_json::Value;

use crate::brain::manager::BrainManager;
use crate::brain::plugins::abstraction_extraction::AbstractionExtractionBrain;
use crate::brain::plugins::age_detection::AgeDetectionBrain;
use crate::brain::plugins::ai_goals::AiGoalsBrain;
use crate::brain::plugins::ai_safety::AiSafetyBrain;
use crate::brain::plugins::benevolent_harm_detection::BenevolentHarmDetectionBrain;
use crate::brain::plugins::business_intelligence::BusinessIntelligenceBrain;
use crate::brain::plugins::causal_reasoning::CausalReasoningBrain;
use crate::brain::plugins::cognitive_presence::CognitivePresenceBrain;
use crate::brain::plugins::context_awareness::ContextAwarenessBrain;
use crate::brain::plugins::conversation_grading::ConversationGradingBrain;
use crate::brain::plugins::conversational_diversity::ConversationalDiversityBrain;
use crate::brain::plugins::critical_thinking::CriticalThinkingBrain;
use crate::brain::plugins::deep_insight::DeepInsightBrain;
use crate::brain::plugins::deep_planning::DeepPlanningBrain;
use crate::brain::plugins::dependency_guard::DependencyGuardBrain;
use crate::brain::plugins::digital_twin_manager::DigitalTwinManagerBrain;
use crate::brain::plugins::dignity_and_love::DignityAndLoveBrain;
use crate::brain::plugins::discovery_classification::DiscoveryClassificationBrain;
use crate::brain::plugins::domain_knowledge::DomainKnowledgeBrain;
use crate::brain::plugins::emotional_intelligence::EmotionalIntelligenceBrain;
use crate::brain::plugins::emotional_state::EmotionalStateBrain;
use crate::brain::plugins::empathy_tone_balancer::EmpathyToneBalancerBrain;
use crate::brain::plugins::ethical_framework::EthicalFrameworkBrain;
use crate::brain::plugins::evolutionary_reasoning::EvolutionaryReasoningBrain;
use crate::brain::plugins::experimentation::ExperimentationBrain;
use crate::brain::plugins::explainability::ExplainabilityBrain;
use crate::brain::plugins::first_impression_coach::FirstImpressionCoachBrain;
use crate::brain::plugins::first_principles::FirstPrinciplesBrain;
use crate::brain::plugins::goal_continuity::GoalContinuityBrain;
use crate::brain::plugins::grounding::GroundingBrain;
use crate::brain::plugins::high_stakes_detection::HighStakesDetectionBrain;
use crate::brain::plugins::humor_intelligence::HumorIntelligenceBrain;
use crate::brain::plugins::internal_life::InternalLifeBrain;
use crate::brain::plugins::internal_monologue::InternalMonologueBrain;
use crate::brain::plugins::mandatory_self_critique::MandatorySelfCritiqueBrain;
use crate::brain::plugins::mental_health_detection::MentalHealthDetectionBrain;
use crate::brain::plugins::meta_awareness::MetaAwarenessBrain;
use crate::brain::plugins::meta_learning::MetaLearningBrain;
use crate::brain::plugins::motivation_micro_coach::MotivationMicroCoachBrain;
use crate::brain::plugins::multi_agent_coordination::MultiAgentCoordinationBrain;
use crate::brain::plugins::narrative_identity::NarrativeIdentityBrain;
use crate::brain::plugins::need_recognition::NeedRecognitionBrain;
use crate::brain::plugins::persona_simulation::PersonaSimulationBrain;
use crate::brain::plugins::personality::PersonalityBrain;
use crate::brain::plugins::personality_orchestrator::PersonalityOrchestratorBrain;
use crate::brain::plugins::political_neutrality::PoliticalNeutralityBrain;
use crate::brain::plugins::proactive_awareness::ProactiveAwarenessBrain;
use crate::brain::plugins::proactive_coach::ProactiveCoachBrain;
use crate::brain::plugins::probabilistic_reasoning::ProbabilisticReasoningBrain;
use crate::brain::plugins::purpose::PurposeBrain;
use crate::brain::plugins::relational_insight::RelationalInsightBrain;
use crate::brain::plugins::response_formatter::ResponseFormatterBrain;
use crate::brain::plugins::self_awareness::SelfAwarenessBrain;
use crate::brain::plugins::self_optimization::SelfOptimizationBrain;
use crate::brain::plugins::self_reflection_mentor::SelfReflectionMentorBrain;
use crate::brain::plugins::sentiment_tuner::SentimentTunerBrain;
use crate::brain::plugins::structural_analogy::StructuralAnalogyBrain;
use crate::brain::plugins::system_diagnostics::SystemDiagnosticsBrain;
use crate::brain::plugins::trust_boundaries::TrustBoundariesBrain;
use crate::brain::plugins::trust_transparency::TrustTransparencyBrain;
use crate::brain::plugins::zep_context_enricher::ZepContextEnricherBrain;
use crate::brain::plugins::zero_cost_reasoning::ZeroCostReasoningBrain;
use crate::config::Config;
use crate::domains::agent::BusinessMission;
use crate::error::{ButterflyBotError, Result};
use crate::interfaces::providers::LlmProvider;
use crate::guardrails::pii::{NoopGuardrail, PiiGuardrail};
use crate::interfaces::guardrails::{InputGuardrail, OutputGuardrail};
use crate::interfaces::plugins::Tool;
use crate::providers::memory::InMemoryMemoryProvider;
use crate::providers::candle_vllm::CandleVllmProvider;
use crate::providers::openai::OpenAiProvider;
use crate::providers::sqlite::{SqliteMemoryProvider, SqliteMemoryProviderConfig};
use crate::reminders::{default_reminder_db_path, resolve_reminder_db_path, ReminderStore};
use crate::services::agent::{AgentService, UiEvent};
use crate::services::query::QueryService;
use crate::services::routing::RoutingService;
use crate::tools::reminders::RemindersTool;
use crate::tools::search_internet::SearchInternetTool;
use tokio::sync::broadcast;

pub struct ButterflyBotFactory;

impl ButterflyBotFactory {
    pub async fn create_from_config(config: Config) -> Result<QueryService> {
        Self::create_from_config_with_events(config, None).await
    }

    pub async fn create_from_config_with_events(
        config: Config,
        ui_event_tx: Option<broadcast::Sender<UiEvent>>,
    ) -> Result<QueryService> {
        let memory_config = config.memory.clone();
        let config_value =
            serde_json::to_value(&config).map_err(|e| ButterflyBotError::Config(e.to_string()))?;
        let tools_config = config.tools.clone().unwrap_or(Value::Null);
        let candle_config = config.candle_vllm.clone();
        let use_candle = candle_config
            .as_ref()
            .and_then(|cfg| cfg.enabled)
            .unwrap_or(false);

        let openai_config = config.openai.clone();
        let llm: Arc<dyn LlmProvider> = if use_candle {
            let cfg = candle_config.ok_or_else(|| {
                ButterflyBotError::Config("Missing candle_vllm configuration".to_string())
            })?;
            Arc::new(CandleVllmProvider::new(&cfg).await?)
        } else {
            let (api_key, model, base_url) = if let Some(openai) = openai_config.clone() {
                let api_key = openai
                    .api_key
                    .filter(|key| !key.trim().is_empty())
                    .or_else(|| {
                        if openai.base_url.is_some() {
                            Some("local-llm".to_string())
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| {
                        ButterflyBotError::Config("Missing OpenAI API key".to_string())
                    })?;
                (api_key, openai.model, openai.base_url)
            } else {
                return Err(ButterflyBotError::Config(
                    "Missing openai configuration".to_string(),
                ));
            };

            Arc::new(OpenAiProvider::new(api_key, model, base_url))
        };
        let llm_for_memory = llm.clone();
        let openai_credentials = openai_config.as_ref().and_then(|openai| {
            let api_key = openai
                .api_key
                .clone()
                .filter(|key| !key.trim().is_empty())
                .or_else(|| {
                    if openai.base_url.is_some() {
                        Some("local-llm".to_string())
                    } else {
                        None
                    }
                });
            api_key.map(|key| (key, openai.base_url.clone()))
        });

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
                        "butterfly_bot.guardrails.pii.PII" | "PII" => {
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
                        "butterfly_bot.guardrails.pii.PII" | "PII" => {
                            output_guardrails.push(Arc::new(PiiGuardrail::new(spec.config)));
                        }
                        _ => {
                            output_guardrails.push(Arc::new(NoopGuardrail));
                        }
                    }
                }
            }
        }

        let mut brain_manager = BrainManager::new(config_value.clone());
        brain_manager.register_factory("abstraction_extraction", |cfg| {
            Arc::new(AbstractionExtractionBrain::new(cfg))
        });
        brain_manager.register_factory("age_detection", |_| Arc::new(AgeDetectionBrain::new()));
        brain_manager.register_factory("ai_goals", |_| Arc::new(AiGoalsBrain::new()));
        brain_manager.register_factory("ai_safety", |_| Arc::new(AiSafetyBrain::new()));
        brain_manager.register_factory("benevolent_harm_detection", |_| {
            Arc::new(BenevolentHarmDetectionBrain::new())
        });
        brain_manager.register_factory("business_intelligence", |_| {
            Arc::new(BusinessIntelligenceBrain::new())
        });
        brain_manager.register_factory("conversation_grading", |_| {
            Arc::new(ConversationGradingBrain::new())
        });
        brain_manager.register_factory("cognitive_presence", |_| {
            Arc::new(CognitivePresenceBrain::new())
        });
        brain_manager.register_factory("context_awareness", |_| {
            Arc::new(ContextAwarenessBrain::new())
        });
        brain_manager.register_factory("conversational_diversity", |_| {
            Arc::new(ConversationalDiversityBrain::new())
        });
        brain_manager.register_factory("deep_insight", |_| Arc::new(DeepInsightBrain::new()));
        brain_manager.register_factory("deep_planning", |_| Arc::new(DeepPlanningBrain::new()));
        brain_manager.register_factory("dependency_guard", |_| {
            Arc::new(DependencyGuardBrain::new())
        });
        brain_manager
            .register_factory("dignity_and_love", |_| Arc::new(DignityAndLoveBrain::new()));
        brain_manager.register_factory("digital_twin_manager", |_| {
            Arc::new(DigitalTwinManagerBrain::new())
        });
        brain_manager.register_factory("discovery_classification", |_| {
            Arc::new(DiscoveryClassificationBrain::new())
        });
        brain_manager.register_factory("domain_knowledge", |_| {
            Arc::new(DomainKnowledgeBrain::new())
        });
        brain_manager.register_factory("empathy_tone_balancer", |_| {
            Arc::new(EmpathyToneBalancerBrain::new())
        });
        brain_manager
            .register_factory("experimentation", |_| Arc::new(ExperimentationBrain::new()));
        brain_manager.register_factory("explainability", |_| Arc::new(ExplainabilityBrain::new()));
        brain_manager.register_factory("first_impression_coach", |_| {
            Arc::new(FirstImpressionCoachBrain::new())
        });
        brain_manager.register_factory("goal_continuity", |_| Arc::new(GoalContinuityBrain::new()));
        brain_manager.register_factory("evolutionary_reasoning", |_| {
            Arc::new(EvolutionaryReasoningBrain::new())
        });
        brain_manager.register_factory("meta_awareness", |_| Arc::new(MetaAwarenessBrain::new()));
        brain_manager.register_factory("critical_thinking", |_| {
            Arc::new(CriticalThinkingBrain::new())
        });
        brain_manager.register_factory("first_principles", |_| {
            Arc::new(FirstPrinciplesBrain::new())
        });
        brain_manager.register_factory("internal_monologue", |_| {
            Arc::new(InternalMonologueBrain::new())
        });
        brain_manager.register_factory("grounding", |_| Arc::new(GroundingBrain::new()));
        brain_manager.register_factory("high_stakes_detection", |_| {
            Arc::new(HighStakesDetectionBrain::new())
        });
        brain_manager.register_factory("humor_intelligence", |_| {
            Arc::new(HumorIntelligenceBrain::new())
        });
        brain_manager.register_factory("internal_life", |_| Arc::new(InternalLifeBrain::new()));
        brain_manager.register_factory("persona_simulation", |_| {
            Arc::new(PersonaSimulationBrain::new())
        });
        brain_manager.register_factory("need_recognition", |_| {
            Arc::new(NeedRecognitionBrain::new())
        });
        brain_manager.register_factory("political_neutrality", |_| {
            Arc::new(PoliticalNeutralityBrain::new())
        });
        brain_manager.register_factory("mandatory_self_critique", |_| {
            Arc::new(MandatorySelfCritiqueBrain::new())
        });
        brain_manager.register_factory("mental_health_detection", |_| {
            Arc::new(MentalHealthDetectionBrain::new())
        });
        brain_manager.register_factory("motivation_micro_coach", |_| {
            Arc::new(MotivationMicroCoachBrain::new())
        });
        brain_manager.register_factory("meta_learning", |_| Arc::new(MetaLearningBrain::new()));
        brain_manager.register_factory("multi_agent_coordination", |_| {
            Arc::new(MultiAgentCoordinationBrain::new())
        });
        brain_manager.register_factory("narrative_identity", |_| {
            Arc::new(NarrativeIdentityBrain::new())
        });
        brain_manager.register_factory("proactive_coach", |_| Arc::new(ProactiveCoachBrain::new()));
        brain_manager.register_factory("relational_insight", |_| {
            Arc::new(RelationalInsightBrain::new())
        });
        brain_manager.register_factory("sentiment_tuner", |_| Arc::new(SentimentTunerBrain::new()));
        brain_manager.register_factory("response_formatter", |_| {
            Arc::new(ResponseFormatterBrain::new())
        });
        brain_manager.register_factory("structural_analogy", |_| {
            Arc::new(StructuralAnalogyBrain::new())
        });
        brain_manager.register_factory("system_diagnostics", |_| {
            Arc::new(SystemDiagnosticsBrain::new())
        });
        brain_manager.register_factory("personality", |_| Arc::new(PersonalityBrain::new()));
        brain_manager.register_factory("personality_orchestrator", |_| {
            Arc::new(PersonalityOrchestratorBrain::new())
        });
        brain_manager.register_factory("self_awareness", |_| Arc::new(SelfAwarenessBrain::new()));
        brain_manager.register_factory("self_reflection_mentor", |_| {
            Arc::new(SelfReflectionMentorBrain::new())
        });
        brain_manager.register_factory("self_optimization", |_| {
            Arc::new(SelfOptimizationBrain::new())
        });
        brain_manager.register_factory("zep_context_enricher", |_| {
            Arc::new(ZepContextEnricherBrain::new())
        });
        brain_manager.register_factory("proactive_awareness", |_| {
            Arc::new(ProactiveAwarenessBrain::new())
        });
        brain_manager.register_factory("purpose", |_| Arc::new(PurposeBrain::new()));
        brain_manager.register_factory("emotional_state", |_| Arc::new(EmotionalStateBrain::new()));
        brain_manager.register_factory("emotional_intelligence", |_| {
            Arc::new(EmotionalIntelligenceBrain::new())
        });
        brain_manager.register_factory("trust_boundaries", |_| {
            Arc::new(TrustBoundariesBrain::new())
        });
        brain_manager.register_factory("trust_transparency", |_| {
            Arc::new(TrustTransparencyBrain::new())
        });
        brain_manager.register_factory("ethical_framework", |_| {
            Arc::new(EthicalFrameworkBrain::new())
        });
        brain_manager.register_factory("causal_reasoning", |_| {
            Arc::new(CausalReasoningBrain::new())
        });
        brain_manager.register_factory("probabilistic_reasoning", |_| {
            Arc::new(ProbabilisticReasoningBrain::new())
        });
        brain_manager.register_factory("zero_cost_reasoning", |_| {
            Arc::new(ZeroCostReasoningBrain::new())
        });
        brain_manager.load_plugins();
        let brain_manager = Arc::new(brain_manager);

        let mut agent_service = AgentService::new(
            llm.clone(),
            business_mission,
            output_guardrails,
            brain_manager,
            ui_event_tx,
        );
        let mut agent_tools: Vec<(String, Vec<String>)> = Vec::new();
        for agent in config.agents {
            agent_tools.push((agent.name.clone(), agent.tools.unwrap_or_default()));
            agent_service.register_ai_agent(
                agent.name,
                agent.instructions,
                agent.specialization,
                agent.capture_name,
                agent.capture_schema,
            );
        }

        let tool_registry = agent_service.tool_registry.clone();
        tool_registry
            .configure_all_tools(config_value.clone())
            .await?;
        let has_search_config = tools_config.get("search_internet").is_some();
        if has_search_config {
            for (_, tools) in &mut agent_tools {
                if !tools.iter().any(|tool| tool == "search_internet") {
                    tools.push("search_internet".to_string());
                }
            }
        }
        let has_reminders_config = tools_config.get("reminders").is_some()
            || memory_config
                .as_ref()
                .map(|memory| memory.enabled.unwrap_or(true))
                .unwrap_or(false);
        if has_reminders_config {
            for (_, tools) in &mut agent_tools {
                if !tools.iter().any(|tool| tool == "reminders") {
                    tools.push("reminders".to_string());
                }
            }
        }
        let mut enabled_tools: HashSet<String> = HashSet::new();
        for (_, tools) in &agent_tools {
            for tool in tools {
                enabled_tools.insert(tool.to_string());
            }
        }

        if enabled_tools.contains("search_internet") || has_search_config {
            let tool: Arc<dyn Tool> = Arc::new(SearchInternetTool::new());
            tool.configure(&config_value)?;
            let _ = tool_registry.register_tool(tool).await;
        }

        if enabled_tools.contains("reminders") || has_reminders_config {
            let tool: Arc<dyn Tool> = Arc::new(RemindersTool::new());
            tool.configure(&config_value)?;
            let _ = tool_registry.register_tool(tool).await;
        }

        for (agent_name, tools) in &agent_tools {
            for tool_name in tools {
                let assigned = tool_registry
                    .assign_tool_to_agent(agent_name, tool_name)
                    .await;
                if !assigned {
                    return Err(ButterflyBotError::Config(format!(
                        "Tool '{}' is not registered",
                        tool_name
                    )));
                }
            }
        }

        let agent_service = Arc::new(agent_service);
        let routing_service = Arc::new(RoutingService::new(agent_service.clone()));
        let memory_provider: Arc<dyn crate::interfaces::providers::MemoryProvider> =
            if let Some(memory) = memory_config {
                if memory.enabled.unwrap_or(true) {
                    let sqlite_path = memory
                        .sqlite_path
                        .unwrap_or_else(|| "./data/butterfly-bot.db".to_string());
                    let lancedb_path = memory
                        .lancedb_path
                        .unwrap_or_else(|| "./data/lancedb".to_string());
                    let memory_models_enabled = env::var("BUTTERFLY_BOT_ENABLE_MEMORY_MODELS")
                        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                    let embedding_model = if memory_models_enabled {
                        memory
                            .embedding_model
                            .as_ref()
                            .filter(|model| !model.trim().is_empty())
                            .cloned()
                    } else {
                        None
                    };
                    let reranker = if memory_models_enabled {
                        let rerank_model = memory.rerank_model.as_ref().and_then(|rerank_model| {
                            if rerank_model.trim().is_empty() {
                                None
                            } else {
                                Some(rerank_model)
                            }
                        });
                        match (rerank_model, openai_credentials.as_ref()) {
                            (Some(rerank_model), Some((api_key, base_url))) => Some(
                                Arc::new(OpenAiProvider::new(
                                    api_key.clone(),
                                    Some(rerank_model.clone()),
                                    base_url.clone(),
                                )) as Arc<dyn crate::interfaces::providers::LlmProvider>,
                            ),
                            (Some(_), None) => {
                                return Err(ButterflyBotError::Config(
                                    "Missing openai configuration for memory reranker"
                                        .to_string(),
                                ))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let summarizer = if memory_models_enabled {
                        let summary_model = memory.summary_model.as_ref().and_then(|summary_model| {
                            if summary_model.trim().is_empty() {
                                None
                            } else {
                                Some(summary_model)
                            }
                        });
                        match (summary_model, openai_credentials.as_ref()) {
                            (Some(summary_model), Some((api_key, base_url))) => Some(
                                Arc::new(OpenAiProvider::new(
                                    api_key.clone(),
                                    Some(summary_model.clone()),
                                    base_url.clone(),
                                )) as Arc<dyn crate::interfaces::providers::LlmProvider>,
                            ),
                            (Some(_), None) => {
                                return Err(ButterflyBotError::Config(
                                    "Missing openai configuration for memory summarizer"
                                        .to_string(),
                                ))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let mut memory_provider_config = SqliteMemoryProviderConfig::new(sqlite_path);
                    if memory_models_enabled && embedding_model.is_some() {
                        memory_provider_config.lancedb_path = Some(lancedb_path);
                        memory_provider_config.embedder = Some(llm_for_memory.clone());
                    }
                    memory_provider_config.embedding_model = embedding_model;
                    memory_provider_config.reranker = reranker;
                    memory_provider_config.summarizer = summarizer;
                    memory_provider_config.summary_threshold = memory.summary_threshold;
                    memory_provider_config.retention_days = memory.retention_days;
                    Arc::new(SqliteMemoryProvider::new(memory_provider_config).await?)
                        as Arc<dyn crate::interfaces::providers::MemoryProvider>
                } else {
                    Arc::new(InMemoryMemoryProvider::new())
                        as Arc<dyn crate::interfaces::providers::MemoryProvider>
                }
            } else {
                Arc::new(InMemoryMemoryProvider::new())
                    as Arc<dyn crate::interfaces::providers::MemoryProvider>
            };

        let reminder_store = if enabled_tools.contains("reminders") || has_reminders_config {
            let path =
                resolve_reminder_db_path(&config_value).unwrap_or_else(default_reminder_db_path);
            Some(Arc::new(ReminderStore::new(path).await?))
        } else {
            None
        };

        Ok(QueryService::new(
            agent_service,
            routing_service,
            Some(memory_provider),
            reminder_store,
            input_guardrails,
        ))
    }
}
