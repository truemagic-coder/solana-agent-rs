use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domains::agent::{AIAgent, BusinessMission};
use crate::error::{Result, SolanaAgentError};
use crate::interfaces::guardrails::OutputGuardrail;
use crate::interfaces::providers::{LlmProvider, ToolCall};
use crate::plugins::registry::ToolRegistry;

pub struct AgentService {
    llm_provider: Arc<dyn LlmProvider>,
    business_mission: Option<BusinessMission>,
    pub tool_registry: Arc<ToolRegistry>,
    agents: Vec<AIAgent>,
    output_guardrails: Vec<Arc<dyn OutputGuardrail>>,
}

impl AgentService {
    pub fn new(
        llm_provider: Arc<dyn LlmProvider>,
        business_mission: Option<BusinessMission>,
        output_guardrails: Vec<Arc<dyn OutputGuardrail>>,
    ) -> Self {
        Self {
            llm_provider,
            business_mission,
            tool_registry: Arc::new(ToolRegistry::new()),
            agents: Vec::new(),
            output_guardrails,
        }
    }

    pub fn register_ai_agent(
        &mut self,
        name: String,
        instructions: String,
        specialization: String,
        capture_name: Option<String>,
        capture_schema: Option<serde_json::Value>,
    ) {
        self.agents.push(AIAgent {
            name,
            instructions,
            specialization,
            capture_name,
            capture_schema,
        });
    }

    pub fn get_all_ai_agents(&self) -> HashMap<String, AIAgent> {
        self.agents
            .iter()
            .cloned()
            .map(|agent| (agent.name.clone(), agent))
            .collect()
    }

    pub fn get_agent_system_prompt(&self, agent_name: &str) -> Result<String> {
        let agent = self
            .agents
            .iter()
            .find(|a| a.name == agent_name)
            .ok_or_else(|| SolanaAgentError::Runtime("Agent not found".to_string()))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| SolanaAgentError::Runtime(e.to_string()))?
            .as_secs();

        let mut system_prompt = format!(
            "You are {}, an AI assistant with the following instructions:\n\n{}\n\nCurrent time (unix seconds): {}",
            agent.name, agent.instructions, now
        );

        if let Some(mission) = &self.business_mission {
            if let Some(m) = &mission.mission {
                system_prompt.push_str(&format!("\n\nBUSINESS MISSION:\n{}", m));
            }
            if let Some(v) = &mission.voice {
                system_prompt.push_str(&format!("\n\nVOICE OF THE BRAND:\n{}", v));
            }
            if !mission.values.is_empty() {
                let values_text = mission
                    .values
                    .iter()
                    .map(|(name, description)| format!("- {}: {}", name, description))
                    .collect::<Vec<_>>()
                    .join("\n");
                system_prompt.push_str(&format!("\n\nBUSINESS VALUES:\n{}", values_text));
            }
            if !mission.goals.is_empty() {
                let goals_text = mission.goals.join("\n- ");
                system_prompt.push_str(&format!("\n\nBUSINESS GOALS:\n- {}", goals_text));
            }
        }

        Ok(system_prompt)
    }

    pub async fn generate_response(
        &self,
        agent_name: &str,
        user_id: &str,
        query: &str,
        memory_context: &str,
        prompt_override: Option<&str>,
    ) -> Result<String> {
        let system_prompt = self.get_agent_system_prompt(agent_name)?;
        let mut full_prompt = String::new();
        if !memory_context.is_empty() {
            full_prompt.push_str("CONVERSATION HISTORY:\n");
            full_prompt.push_str(memory_context);
            full_prompt.push_str("\n\n");
        }
        if let Some(prompt) = prompt_override {
            full_prompt.push_str("ADDITIONAL PROMPT:\n");
            full_prompt.push_str(prompt);
            full_prompt.push_str("\n\n");
        }
        full_prompt.push_str(query);
        full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

        let tools = self.tool_registry.get_agent_tools(agent_name).await;
        let output = if tools.is_empty() {
            self.llm_provider
                .generate_text(&full_prompt, &system_prompt, None)
                .await?
        } else {
            self.run_tool_loop(&system_prompt, &full_prompt, tools)
                .await?
        };

        let mut processed_output = output;
        for guardrail in &self.output_guardrails {
            processed_output = guardrail.process(&processed_output).await?;
        }

        Ok(processed_output)
    }

    pub async fn generate_response_with_images(
        &self,
        agent_name: &str,
        user_id: &str,
        query: &str,
        images: Vec<crate::interfaces::providers::ImageInput>,
        memory_context: &str,
        prompt_override: Option<&str>,
        detail: &str,
    ) -> Result<String> {
        let system_prompt = self.get_agent_system_prompt(agent_name)?;
        let mut full_prompt = String::new();
        if !memory_context.is_empty() {
            full_prompt.push_str("CONVERSATION HISTORY:\n");
            full_prompt.push_str(memory_context);
            full_prompt.push_str("\n\n");
        }
        if let Some(prompt) = prompt_override {
            full_prompt.push_str("ADDITIONAL PROMPT:\n");
            full_prompt.push_str(prompt);
            full_prompt.push_str("\n\n");
        }
        full_prompt.push_str(query);
        full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

        let output = self
            .llm_provider
            .generate_text_with_images(&full_prompt, images, &system_prompt, detail, None)
            .await?;

        let mut processed_output = output;
        for guardrail in &self.output_guardrails {
            processed_output = guardrail.process(&processed_output).await?;
        }
        Ok(processed_output)
    }

    pub async fn generate_structured_response(
        &self,
        agent_name: &str,
        user_id: &str,
        query: &str,
        memory_context: &str,
        prompt_override: Option<&str>,
        json_schema: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let system_prompt = self.get_agent_system_prompt(agent_name)?;
        let mut full_prompt = String::new();
        if !memory_context.is_empty() {
            full_prompt.push_str("CONVERSATION HISTORY:\n");
            full_prompt.push_str(memory_context);
            full_prompt.push_str("\n\n");
        }
        if let Some(prompt) = prompt_override {
            full_prompt.push_str("ADDITIONAL PROMPT:\n");
            full_prompt.push_str(prompt);
            full_prompt.push_str("\n\n");
        }
        full_prompt.push_str(query);
        full_prompt.push_str(&format!("\n\nUSER IDENTIFIER: {}", user_id));

        self.llm_provider
            .parse_structured_output(&full_prompt, &system_prompt, json_schema, None)
            .await
    }

    pub async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        input_format: &str,
    ) -> Result<String> {
        self.llm_provider
            .transcribe_audio(audio_bytes, input_format)
            .await
    }

    pub async fn synthesize_audio(
        &self,
        text: &str,
        voice: &str,
        response_format: &str,
    ) -> Result<Vec<u8>> {
        self.llm_provider.tts(text, voice, response_format).await
    }

    async fn run_tool_loop(
        &self,
        system_prompt: &str,
        initial_prompt: &str,
        tools: Vec<Arc<dyn crate::interfaces::plugins::Tool>>,
    ) -> Result<String> {
        let mut prompt = initial_prompt.to_string();
        let mut last_text = String::new();
        let mut tool_specs = Vec::new();

        for tool in &tools {
            tool_specs.push(serde_json::json!({
                "type": "function",
                "name": tool.name(),
                "description": tool.description(),
                "parameters": tool.parameters(),
            }));
        }

        for _ in 0..5 {
            let response = self
                .llm_provider
                .generate_with_tools(&prompt, system_prompt, tool_specs.clone())
                .await?;
            if !response.text.is_empty() {
                last_text = response.text.clone();
            }
            if response.tool_calls.is_empty() {
                return Ok(last_text);
            }

            let results = self
                .execute_tool_calls(&response.tool_calls, &tools)
                .await?;
            let serialized = serde_json::to_string_pretty(&results)
                .map_err(|e| SolanaAgentError::Serialization(e.to_string()))?;
            prompt.push_str("\n\nTOOL_RESULTS:\n");
            prompt.push_str(&serialized);
        }

        Ok(last_text)
    }

    async fn execute_tool_calls(
        &self,
        calls: &[ToolCall],
        tools: &[Arc<dyn crate::interfaces::plugins::Tool>],
    ) -> Result<Vec<serde_json::Value>> {
        let mut results = Vec::new();
        for call in calls {
            let tool = tools.iter().find(|t| t.name() == call.name);
            match tool {
                Some(tool) => {
                    let result = tool.execute(call.arguments.clone()).await?;
                    results.push(serde_json::json!({
                        "tool": call.name,
                        "status": "success",
                        "result": result,
                    }));
                }
                None => {
                    results.push(serde_json::json!({
                        "tool": call.name,
                        "status": "error",
                        "message": "Tool not found",
                    }));
                }
            }
        }
        Ok(results)
    }
}
