use crate::tools::tool_trait::{Tool, ToolContext};
use async_trait::async_trait;
use clawz_core::error::ClawzError;
use clawz_core::types::tool_risk::{ActionPrimitive, RiskLevel};
use clawz_core::types::{ToolResult, ToolSchema};
use serde_json::Value;

pub struct ImageGenTool;

impl ImageGenTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ImageGenTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ImageGenTool {
    fn name(&self) -> &str {
        "image_gen"
    }

    fn description(&self) -> &str {
        "Generate images from text prompts using DALL-E (OpenAI) or Stable Diffusion. Returns a URL or base64-encoded PNG."
    }

    fn primitive(&self) -> ActionPrimitive {
        ActionPrimitive::Transform
    }
    fn risk(&self) -> RiskLevel {
        RiskLevel::Medium
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "image_gen".into(),
            description: self.description().into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Text description of the image to generate"
                    },
                    "model": {
                        "type": "string",
                        "enum": ["dall-e-3", "dall-e-2", "stable-diffusion"],
                        "description": "Image generation model (default: dall-e-3)"
                    },
                    "size": {
                        "type": "string",
                        "enum": ["256x256", "512x512", "1024x1024", "1024x1792", "1792x1024"],
                        "description": "Image size (default: 1024x1024)"
                    },
                    "quality": {
                        "type": "string",
                        "enum": ["standard", "hd"],
                        "description": "Image quality for DALL-E 3 (default: standard)"
                    },
                    "style": {
                        "type": "string",
                        "enum": ["vivid", "natural"],
                        "description": "Style for DALL-E 3 (default: vivid)"
                    },
                    "n": {
                        "type": "integer",
                        "description": "Number of images to generate (default: 1, max: 4)"
                    },
                    "response_format": {
                        "type": "string",
                        "enum": ["url", "b64_json"],
                        "description": "Response format (default: url)"
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult, ClawzError> {
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| ClawzError::Validation("prompt required".into()))?;

        let model = args["model"].as_str().unwrap_or("dall-e-3");
        let size = args["size"].as_str().unwrap_or("1024x1024");
        let quality = args["quality"].as_str().unwrap_or("standard");
        let style = args["style"].as_str().unwrap_or("vivid");
        let n = args["n"].as_u64().unwrap_or(1).min(4);
        let response_format = args["response_format"].as_str().unwrap_or("url");

        match model {
            "stable-diffusion" => generate_stable_diffusion(prompt, size, n, response_format).await,
            _ => generate_dalle(prompt, model, size, quality, style, n, response_format).await,
        }
    }
}

async fn generate_dalle(
    prompt: &str,
    model: &str,
    size: &str,
    quality: &str,
    style: &str,
    n: u64,
    response_format: &str,
) -> Result<ToolResult, ClawzError> {
    let api_key = std::env::var("OPENAI_API_KEY")
        .map_err(|_| ClawzError::Config("OPENAI_API_KEY environment variable not set".into()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| ClawzError::Tool(format!("HTTP client build failed: {e}")))?;

    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "n": n,
        "size": size,
        "response_format": response_format
    });

    // DALL-E 3 specific params
    if model == "dall-e-3" {
        body["quality"] = Value::String(quality.into());
        body["style"] = Value::String(style.into());
    }

    let response = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(&api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ClawzError::Tool(format!("DALL-E API request failed: {e}")))?;

    let status = response.status();
    let resp_body: Value = response
        .json()
        .await
        .map_err(|e| ClawzError::Tool(format!("failed to parse DALL-E response: {e}")))?;

    if !status.is_success() {
        let error_msg = resp_body["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(ClawzError::Tool(format!(
            "DALL-E API error ({}): {}",
            status.as_u16(),
            error_msg
        )));
    }

    let images: Vec<Value> = resp_body["data"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|img| {
            if response_format == "url" {
                serde_json::json!({
                    "url": img["url"].as_str().unwrap_or(""),
                    "revised_prompt": img["revised_prompt"].as_str().unwrap_or("")
                })
            } else {
                serde_json::json!({
                    "b64_json": img["b64_json"].as_str().unwrap_or(""),
                    "revised_prompt": img["revised_prompt"].as_str().unwrap_or("")
                })
            }
        })
        .collect();

    let output = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "images": images,
        "count": images.len()
    })
    .to_string();

    Ok(ToolResult {
        tool_call_id: String::new(),
        output,
        is_error: false,
    })
}

async fn generate_stable_diffusion(
    prompt: &str,
    size: &str,
    n: u64,
    response_format: &str,
) -> Result<ToolResult, ClawzError> {
    let api_key = std::env::var("STABILITY_API_KEY")
        .or_else(|_| std::env::var("SD_API_KEY"))
        .map_err(|_| {
            ClawzError::Config(
                "STABILITY_API_KEY or SD_API_KEY environment variable not set".into(),
            )
        })?;

    let base_url =
        std::env::var("SD_API_URL").unwrap_or_else(|_| "https://api.stability.ai".into());

    let (width, height) = parse_size(size);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| ClawzError::Tool(format!("HTTP client build failed: {e}")))?;

    let body = serde_json::json!({
        "text_prompts": [{ "text": prompt, "weight": 1.0 }],
        "width": width,
        "height": height,
        "samples": n,
        "steps": 30
    });

    let url = format!("{base_url}/v1/generation/stable-diffusion-xl-1024-v1-0/text-to-image");
    let response = client
        .post(&url)
        .bearer_auth(&api_key)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ClawzError::Tool(format!("Stability AI request failed: {e}")))?;

    let status = response.status();
    let resp_body: Value = response
        .json()
        .await
        .map_err(|e| ClawzError::Tool(format!("failed to parse SD response: {e}")))?;

    if !status.is_success() {
        return Err(ClawzError::Tool(format!(
            "Stability AI error ({}): {}",
            status.as_u16(),
            resp_body
        )));
    }

    let artifacts: Vec<Value> = resp_body["artifacts"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|a| {
            if response_format == "b64_json" {
                serde_json::json!({
                    "b64_json": a["base64"].as_str().unwrap_or(""),
                    "finish_reason": a["finishReason"].as_str().unwrap_or("")
                })
            } else {
                // For SD we only have b64, but we can present it as data URL
                let b64 = a["base64"].as_str().unwrap_or("");
                serde_json::json!({
                    "url": format!("data:image/png;base64,{}", b64),
                    "finish_reason": a["finishReason"].as_str().unwrap_or("")
                })
            }
        })
        .collect();

    let output = serde_json::json!({
        "model": "stable-diffusion",
        "prompt": prompt,
        "images": artifacts,
        "count": artifacts.len()
    })
    .to_string();

    Ok(ToolResult {
        tool_call_id: String::new(),
        output,
        is_error: false,
    })
}

fn parse_size(size: &str) -> (u32, u32) {
    let parts: Vec<&str> = size.splitn(2, 'x').collect();
    if parts.len() == 2 {
        let w = parts[0].parse().unwrap_or(1024);
        let h = parts[1].parse().unwrap_or(1024);
        (w, h)
    } else {
        (1024, 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolConfig;

    fn make_ctx() -> ToolContext {
        ToolContext {
            agent_id: "a".into(),
            conversation_id: "c".into(),
            user_id: None,
            config: ToolConfig::default(),
        }
    }

    #[test]
    fn test_image_gen_name() {
        assert_eq!(ImageGenTool::new().name(), "image_gen");
    }

    #[test]
    fn test_image_gen_schema() {
        let schema = ImageGenTool::new().schema();
        assert_eq!(schema.name, "image_gen");
        assert!(schema.parameters["properties"]["prompt"].is_object());
    }

    #[tokio::test]
    async fn test_missing_prompt() {
        let tool = ImageGenTool::new();
        let ctx = make_ctx();
        let result = tool.execute(&ctx, serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_api_key() {
        // Without OPENAI_API_KEY set, should fail with Config error
        let tool = ImageGenTool::new();
        let ctx = make_ctx();
        // Only test if key is not set
        if std::env::var("OPENAI_API_KEY").is_err() {
            let result = tool
                .execute(&ctx, serde_json::json!({"prompt": "a red circle"}))
                .await;
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("512x512"), (512, 512));
        assert_eq!(parse_size("1024x1792"), (1024, 1792));
        assert_eq!(parse_size("invalid"), (1024, 1024));
    }
}
