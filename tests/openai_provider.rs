use futures::StreamExt;
use httpmock::Method::POST;
use httpmock::MockServer;
use serde_json::json;

use butterfly_bot::client::ButterflyBot;
use butterfly_bot::config::{Config, OpenAiConfig};
use butterfly_bot::error::ButterflyBotError;
use butterfly_bot::interfaces::providers::{ImageData, ImageInput, LlmProvider};
use butterfly_bot::providers::openai::OpenAiProvider;
use butterfly_bot::services::query::{OutputFormat, ProcessOptions, ProcessResult, UserInput};

#[tokio::test]
async fn openai_provider_via_httpmock() {
    let server = MockServer::start_async().await;
    let chat_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-1",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "hello"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(server.base_url()),
    );
    let text = provider.generate_text("hi", "", None).await.unwrap();
    assert_eq!(text, "hello");

    let mut stream = provider.chat_stream(vec![json!({"role":"user","content":"hi"})], None);
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.event_type, "content");
    let last = stream.next().await.unwrap().unwrap();
    assert_eq!(last.event_type, "message_end");

    chat_mock.assert_hits(2);
}

#[tokio::test]
async fn openai_provider_tools_images_structured_audio() {
    let server = MockServer::start_async().await;

    let tool_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-2",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "type": "function",
                                "id": "call_1",
                                "function": {"name": "tool1", "arguments": "{\"x\":1}"}
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            }));
        })
        .await;

    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(server.base_url()),
    );
    let response = provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"tool1","parameters":{}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "tool1");

    tool_mock.assert_hits(1);

    let structured_server = MockServer::start_async().await;
    let structured_mock = structured_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-3",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "{\"ok\":true}"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let structured_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(structured_server.base_url()),
    );
    let structured = structured_provider
        .parse_structured_output("hi", "", json!({"type":"object"}), None)
        .await
        .unwrap();
    assert_eq!(structured, json!({"ok": true}));
    structured_mock.assert_hits(1);

    let image_server = MockServer::start_async().await;
    let image_mock = image_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-4",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "image"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let image_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(image_server.base_url()),
    );
    let image_text = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Bytes(vec![1, 2, 3]),
            }],
            "",
            "high",
            None,
        )
        .await
        .unwrap();
    assert_eq!(image_text, "image");
    image_mock.assert_hits(1);

    let speech_server = MockServer::start_async().await;
    let speech_mock = speech_server
        .mock_async(|when, then| {
            when.method(POST).path("/audio/speech");
            then.status(200).body("AUDIO");
        })
        .await;
    let speech_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(speech_server.base_url()),
    );
    let audio = speech_provider.tts("hello", "alloy", "mp3").await.unwrap();
    assert_eq!(audio, b"AUDIO".to_vec());
    speech_mock.assert_hits(1);

    let transcribe_server = MockServer::start_async().await;
    let transcribe_mock = transcribe_server
        .mock_async(|when, then| {
            when.method(POST).path("/audio/transcriptions");
            then.status(200).json_body(json!({
                "text": "transcribed",
                "logprobs": null,
                "usage": {
                    "type": "tokens",
                    "input_tokens": 1,
                    "output_tokens": 1,
                    "total_tokens": 2,
                    "input_token_details": null
                }
            }));
        })
        .await;
    let transcribe_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(transcribe_server.base_url()),
    );
    let transcript = transcribe_provider
        .transcribe_audio(vec![1, 2, 3], "wav")
        .await
        .unwrap();
    assert_eq!(transcript, "transcribed");
    transcribe_mock.assert_hits(1);
}

#[tokio::test]
async fn openai_provider_additional_branches() {
    let tools_server = MockServer::start_async().await;
    let tools_mock = tools_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-tools",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "with tools"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let tools_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(tools_server.base_url()),
    );
    let text = tools_provider
        .generate_text(
            "hi",
            "sys",
            Some(vec![
                json!({"type":"function","name":"tool1","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(text, "with tools");
    tools_mock.assert_hits(1);

    let empty_server = MockServer::start_async().await;
    let empty_mock = empty_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-empty",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": []
            }));
        })
        .await;
    let empty_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(empty_server.base_url()),
    );
    let response = empty_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"tool1","parameters":{}})],
        )
        .await
        .unwrap();
    assert!(response.text.is_empty());
    assert!(response.tool_calls.is_empty());
    empty_mock.assert_hits(1);

    let structured_server = MockServer::start_async().await;
    let structured_mock = structured_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-struct-tools",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "{\"ok\":true}"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let structured_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(structured_server.base_url()),
    );
    let structured = structured_provider
        .parse_structured_output(
            "hi",
            "system",
            json!({"title":"Example","type":"object"}),
            Some(vec![
                json!({"type":"function","name":"tool1","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(structured, json!({"ok": true}));
    structured_mock.assert_hits(1);

    let image_server = MockServer::start_async().await;
    let image_mock = image_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-image-tools",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "image tools"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let image_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(image_server.base_url()),
    );
    let image_text = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Bytes(vec![1, 2, 3]),
            }],
            "sys",
            "auto",
            Some(vec![
                json!({"type":"function","name":"tool1","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(image_text, "image tools");
    image_mock.assert_hits(1);
}

#[tokio::test]
async fn openai_provider_variants_and_agent_process() {
    let chat_server = MockServer::start_async().await;
    let chat_mock = chat_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-5",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "text"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let chat_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(chat_server.base_url()),
    );
    let text = chat_provider
        .generate_text("hi", "", Some(vec![json!({"type":"custom","name":"x"})]))
        .await
        .unwrap();
    assert_eq!(text, "text");
    chat_mock.assert_hits(1);

    let skip_server = MockServer::start_async().await;
    let skip_mock = skip_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-skip",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "skip"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let skip_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(skip_server.base_url()),
    );
    let text = skip_provider
        .generate_text(
            "hi",
            "sys",
            Some(vec![
                json!({"type":"custom","name":"x"}),
                json!({"type":"function","parameters":{}}),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(text, "skip");
    skip_mock.assert_hits(1);

    let nested_server = MockServer::start_async().await;
    let nested_mock = nested_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-6",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "type": "function",
                                "id": "call_1",
                                "function": {"name": "tool_nested", "arguments": "{\"x\":1}"}
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            }));
        })
        .await;
    let nested_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(nested_server.base_url()),
    );
    let response = nested_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","function":{"name":"tool_nested","parameters":{}}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].name, "tool_nested");
    nested_mock.assert_hits(1);

    let custom_server = MockServer::start_async().await;
    let custom_mock = custom_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-7",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "type": "custom",
                                "id": "call_2",
                                "custom_tool": {"name": "custom_tool", "input": "{\"y\":2}"}
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }]
            }));
        })
        .await;
    let custom_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(custom_server.base_url()),
    );
    let response = custom_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"x","parameters":{}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].name, "custom_tool");
    custom_mock.assert_hits(1);

    let fallback_server = MockServer::start_async().await;
    let fallback_mock = fallback_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-8",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "function_call": {"name": "legacy", "arguments": "{\"z\":3}"}
                    },
                    "finish_reason": "function_call"
                }]
            }));
        })
        .await;
    let fallback_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(fallback_server.base_url()),
    );
    let response = fallback_provider
        .generate_with_tools(
            "hi",
            "sys",
            vec![json!({"type":"function","name":"legacy","parameters":{}})],
        )
        .await
        .unwrap();
    assert_eq!(response.tool_calls[0].name, "legacy");
    fallback_mock.assert_hits(1);

    let image_server = MockServer::start_async().await;
    let image_mock = image_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-9",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "image"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;
    let image_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(image_server.base_url()),
    );
    let _ = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Url("http://example.com".to_string()),
            }],
            "",
            "low",
            None,
        )
        .await
        .unwrap();
    let _ = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Url("http://example.com".to_string()),
            }],
            "sys",
            "weird",
            None,
        )
        .await
        .unwrap();
    let _ = image_provider
        .generate_text_with_images(
            "hi",
            vec![ImageInput {
                data: ImageData::Bytes(vec![1, 2, 3]),
            }],
            "",
            "auto",
            None,
        )
        .await
        .unwrap();
    image_mock.assert_hits(3);

    let speech_server = MockServer::start_async().await;
    let speech_mock = speech_server
        .mock_async(|when, then| {
            when.method(POST).path("/audio/speech");
            then.status(200).body("AUDIO");
        })
        .await;
    let speech_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(speech_server.base_url()),
    );
    let voices = [
        "alloy", "ash", "ballad", "coral", "echo", "fable", "onyx", "nova", "sage", "shimmer",
        "verse", "custom",
    ];
    for voice in voices {
        let _ = speech_provider.tts("hi", voice, "mp3").await.unwrap();
    }
    let formats = ["opus", "aac", "flac", "wav", "pcm", "pcm16", "mp3"];
    for format in formats {
        let _ = speech_provider.tts("hi", "alloy", format).await.unwrap();
    }
    speech_mock.assert_hits(voices.len() + formats.len());

    let agent_server = MockServer::start_async().await;
    let agent_mock = agent_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-10",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "agent response"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let config = Config {
        openai: Some(OpenAiConfig {
            api_key: Some("key".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            base_url: Some(agent_server.base_url()),
        }),
        skill_file: None,
        heartbeat_file: None,
        memory: None,
        tools: None,
        brains: None,
    };
    let agent = ButterflyBot::from_config(config).await.unwrap();
    let result = agent
        .process(
            "user",
            UserInput::Text("hi".to_string()),
            ProcessOptions {
                prompt: None,
                images: vec![],
                output_format: OutputFormat::Text,
                image_detail: "auto".to_string(),
                json_schema: None,
            },
        )
        .await
        .unwrap();
    match result {
        ProcessResult::Text(value) => assert_eq!(value, "agent response"),
        other => panic!("unexpected result: {other:?}"),
    }
    let mut stream = agent.process_text_stream("user", "hi", None);
    let chunk = stream.next().await.unwrap().unwrap();
    assert_eq!(chunk, "agent response");
    agent_mock.assert_hits(2);
}

#[tokio::test]
async fn openai_provider_error_paths() {
    let server = MockServer::start_async().await;
    let empty_mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-err",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": []
            }));
        })
        .await;

    let provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(server.base_url()),
    );
    let err = provider.generate_text("hi", "", None).await.unwrap_err();
    assert!(matches!(err, ButterflyBotError::Runtime(_)));
    empty_mock.assert_hits(1);

    let bad_server = MockServer::start_async().await;
    let bad_mock = bad_server
        .mock_async(|when, then| {
            when.method(POST).path("/chat/completions");
            then.status(200).json_body(json!({
                "id": "chatcmpl-bad",
                "object": "chat.completion",
                "created": 1,
                "model": "gpt-4o-mini",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "not-json"},
                    "finish_reason": "stop"
                }]
            }));
        })
        .await;

    let bad_provider = OpenAiProvider::new(
        "key".to_string(),
        Some("gpt-4o-mini".to_string()),
        Some(bad_server.base_url()),
    );
    let err = bad_provider
        .parse_structured_output("hi", "", json!({"type":"object"}), None)
        .await
        .unwrap_err();
    assert!(matches!(err, ButterflyBotError::Serialization(_)));
    bad_mock.assert_hits(1);
}
