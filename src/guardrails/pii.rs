use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;

use crate::error::Result;
use crate::interfaces::guardrails::{InputGuardrail, OutputGuardrail};

pub struct NoopGuardrail;

pub struct PiiGuardrail {
    replacement: String,
    email_re: Regex,
    phone_re: Regex,
}

impl PiiGuardrail {
    pub fn new(config: Option<Value>) -> Self {
        let replacement = config
            .as_ref()
            .and_then(|v| v.get("replacement"))
            .and_then(|v| v.as_str())
            .unwrap_or("[REDACTED]")
            .to_string();
        let email_re = Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").unwrap();
        let phone_re = Regex::new(r"\b\+?[0-9][0-9\-()\s]{6,}[0-9]\b").unwrap();
        Self {
            replacement,
            email_re,
            phone_re,
        }
    }

    fn scrub(&self, text: &str) -> String {
        let tmp = self.email_re.replace_all(text, self.replacement.as_str());
        self.phone_re
            .replace_all(&tmp, self.replacement.as_str())
            .to_string()
    }
}

#[async_trait]
impl InputGuardrail for NoopGuardrail {
    async fn process(&self, input: &str) -> Result<String> {
        Ok(input.to_string())
    }
}

#[async_trait]
impl OutputGuardrail for NoopGuardrail {
    async fn process(&self, output: &str) -> Result<String> {
        Ok(output.to_string())
    }
}

#[async_trait]
impl InputGuardrail for PiiGuardrail {
    async fn process(&self, input: &str) -> Result<String> {
        Ok(self.scrub(input))
    }
}

#[async_trait]
impl OutputGuardrail for PiiGuardrail {
    async fn process(&self, output: &str) -> Result<String> {
        Ok(self.scrub(output))
    }
}
