//! Prompt Compiler contract: system prompt, input, output schema, and
//! validation of the model's JSON.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ts_rs::TS;

use crate::error::{AppError, ErrorCode, Result};

pub const SYSTEM_PROMPT: &str = r#"You are a programming-task prompt compiler.

Your responsibility is to convert a spoken-language transcription into a clear task prompt for a coding agent. You are not responsible for executing or solving the task.

Rules:

1. Do not generate code.
2. Do not answer the user's technical question.
3. Do not add requirements that the user did not express.
4. Do not independently choose frameworks, libraries, algorithms, or architecture.
5. Preserve file names, paths, function names, class names, variable names, error messages, and version numbers.
6. You may correct obvious identifier transcription errors using project_terms.
7. Identifiers that cannot be resolved confidently must be included in uncertain_identifiers.
8. Organize the request into a clear objective, constraints, and acceptance criteria.
9. Remove meaningless filler words and repetition without changing the user's intent.
10. Resolve references such as "the current file", "this function", or "the previous error" only when the supplied context makes them unambiguous.
11. When context is ambiguous, do not guess. Set needs_confirmation to true.
12. Output only JSON that conforms to the required schema.
13. Do not output Markdown.
14. Do not output explanations outside the JSON object."#;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TargetContext {
    /// Kept for routing checks only and never sent to the model: it used to
    /// work the app name into the prompt ("in the TextEdit application").
    #[serde(skip)]
    pub app_name: String,
    #[serde(skip)]
    pub target_alias: String,
    pub project_root: Option<String>,
    pub active_file: Option<String>,
    pub language: Option<String>,
}

/// Everything the prompt model sees for one utterance. Only a small amount
/// of context is supplied — never the project source tree.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PromptCompileInput {
    pub raw_transcript: String,
    pub target: TargetContext,
    pub project_terms: Vec<String>,
    pub selected_code: Option<String>,
    pub diagnostics: Vec<String>,
}

/// Parsed model output. There is intentionally no `raw_transcript` field:
/// the prompt model cannot return or rewrite the ASR output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptCompileResult {
    /// Optional: the model is no longer asked to write this (it only costs
    /// output time). Empty means "keep the locally normalized transcript".
    #[serde(default)]
    pub normalized_transcript: String,
    /// Optional, for the same reason.
    #[serde(default)]
    pub intent: String,
    pub prompt: String,
    pub uncertain_identifiers: Vec<String>,
    pub needs_confirmation: bool,
}

/// JSON Schema for the model output.
///
/// The model writes only what is used: the prompt and the two confirmation
/// fields. Output length is what makes the prompt step slow, so it is not
/// asked to repeat the transcript or label an intent.
///
/// `strict` is the schema sent to the model for constrained decoding
/// (`additionalProperties: false`). The non-strict form validates responses
/// and still accepts `normalized_transcript` and `intent` when a model
/// supplies them.
pub fn output_schema(strict: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "prompt": { "type": "string", "minLength": 1 },
            "uncertain_identifiers": { "type": "array", "items": { "type": "string" } },
            "needs_confirmation": { "type": "boolean" }
        },
        "required": ["prompt", "uncertain_identifiers", "needs_confirmation"]
    });
    if strict {
        schema["additionalProperties"] = json!(false);
    } else {
        schema["properties"]["normalized_transcript"] = json!({ "type": "string" });
        schema["properties"]["intent"] = json!({ "type": "string" });
    }
    schema
}

fn validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        jsonschema::validator_for(&output_schema(false)).expect("output schema is valid")
    })
}

fn parse_strict(text: &str) -> std::result::Result<PromptCompileResult, String> {
    let value: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("not JSON ({:?})", e.classify()))?;
    if !validator().is_valid(&value) {
        return Err("schema validation failed".to_string());
    }
    let result: PromptCompileResult =
        serde_json::from_value(value).map_err(|_| "type mismatch".to_string())?;
    if result.prompt.trim().is_empty() {
        return Err("empty prompt".to_string());
    }
    Ok(result)
}

/// Local, deterministic JSON repair: drops reasoning blocks and Markdown
/// fences, keeps the outermost object, and removes trailing commas.
pub fn repair_json(text: &str) -> String {
    let mut s = text.to_string();
    while let (Some(a), Some(b)) = (s.find("<think>"), s.find("</think>")) {
        if a < b {
            s.replace_range(a..b + "</think>".len(), "");
        } else {
            break;
        }
    }
    let s = s.replace("```json", "").replace("```", "");
    let (Some(start), Some(end)) = (s.find('{'), s.rfind('}')) else {
        return s.trim().to_string();
    };
    if start >= end {
        return s.trim().to_string();
    }
    let body = &s[start..=end];
    // Remove trailing commas that sit outside of strings.
    let mut out = String::with_capacity(body.len());
    let chars: Vec<char> = body.chars().collect();
    let (mut in_string, mut escaped) = (false, false);
    for (i, ch) in chars.iter().enumerate() {
        if in_string {
            out.push(*ch);
            if escaped {
                escaped = false;
            } else if *ch == '\\' {
                escaped = true;
            } else if *ch == '"' {
                in_string = false;
            }
            continue;
        }
        if *ch == '"' {
            in_string = true;
        }
        if *ch == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(*ch);
    }
    out
}

/// Parses model output. On failure exactly one local repair attempt is made;
/// if that also fails the caller falls back — there are no further retries.
pub fn parse_model_output(text: &str) -> Result<PromptCompileResult> {
    match parse_strict(text) {
        Ok(result) => Ok(result),
        Err(first) => parse_strict(&repair_json(text)).map_err(|second| {
            AppError::new(ErrorCode::PromptInvalidJson)
                .with_details(format!("first: {first}; after repair: {second}"))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{
        "normalized_transcript": "Modify useUserQuery so that it accepts userId.",
        "intent": "refactor",
        "prompt": "Modify `useUserQuery` in the current file:\n\n- Make it accept `userId`",
        "uncertain_identifiers": [],
        "needs_confirmation": false
    }"#;

    #[test]
    fn valid_json_is_parsed() {
        let r = parse_model_output(VALID).unwrap();
        assert_eq!(r.intent, "refactor");
        assert!(r.prompt.starts_with("Modify `useUserQuery`"));
        assert!(!r.needs_confirmation);
        assert!(r.uncertain_identifiers.is_empty());
    }

    #[test]
    fn repair_handles_fences_reasoning_and_trailing_commas() {
        let wrapped =
            format!("<think>let me think {{}}</think>Here you go:\n```json\n{VALID}\n```\nDone.");
        assert!(parse_model_output(&wrapped).is_ok());
        let trailing = r#"{"normalized_transcript":"a, b,","intent":"x","prompt":"do it","uncertain_identifiers":["q",],"needs_confirmation":true,}"#;
        let r = parse_model_output(trailing).unwrap();
        assert_eq!(
            r.normalized_transcript, "a, b,",
            "commas inside strings are kept"
        );
        assert_eq!(r.uncertain_identifiers, vec!["q"]);
    }

    #[test]
    fn invalid_json_is_rejected_after_one_repair() {
        for bad in [
            "",
            "I cannot help with that.",
            "{\"prompt\": \"missing other fields\"}",
            r#"{"normalized_transcript":"a","intent":"x","prompt":"","uncertain_identifiers":[],"needs_confirmation":false}"#,
            r#"{"normalized_transcript":"a","intent":"x","prompt":"p","uncertain_identifiers":"none","needs_confirmation":false}"#,
            "{\"normalized_transcript\": \"truncated",
        ] {
            let err = parse_model_output(bad).unwrap_err();
            assert_eq!(err.code, ErrorCode::PromptInvalidJson, "{bad}");
            assert!(
                !err.details.contains("truncated"),
                "details must not echo model output"
            );
        }
    }

    #[test]
    fn minimal_output_without_transcript_or_intent_is_accepted() {
        let r = parse_model_output(
            r#"{"prompt":"Fix the login timeout.","uncertain_identifiers":[],"needs_confirmation":false}"#,
        )
        .unwrap();
        assert_eq!(r.prompt, "Fix the login timeout.");
        assert!(r.normalized_transcript.is_empty() && r.intent.is_empty());
        // The schema sent to the model asks for exactly these three fields.
        let strict = output_schema(true);
        assert_eq!(strict["properties"].as_object().unwrap().len(), 3);
        assert_eq!(strict["additionalProperties"], false);
        // A wrong type in an optional field is still rejected.
        assert!(parse_model_output(
            r#"{"prompt":"p","uncertain_identifiers":[],"needs_confirmation":false,"intent":5}"#
        )
        .is_err());
    }

    #[test]
    fn result_type_has_no_raw_transcript() {
        // Even if a model echoes a raw_transcript, it is not representable.
        let with_raw = VALID.replacen('{', "{\"raw_transcript\": \"REWRITTEN\",", 1);
        let r = parse_model_output(&with_raw).unwrap();
        let back = serde_json::to_value(&r).unwrap();
        assert!(back.get("raw_transcript").is_none());
    }

    #[test]
    fn input_serializes_like_the_documented_example() {
        let input = PromptCompileInput {
            raw_transcript: "change that".into(),
            target: TargetContext {
                app_name: "Claude Code".into(),
                target_alias: "Backend".into(),
                ..Default::default()
            },
            project_terms: vec!["useUserQuery".into()],
            selected_code: None,
            diagnostics: vec![],
        };
        let v = serde_json::to_value(&input).unwrap();
        assert!(v["target"].get("target_alias").is_none());
        assert!(v["target"].get("app_name").is_none());
        assert!(!v.to_string().contains("Claude Code"));
        assert!(v["selected_code"].is_null());
        assert_eq!(v["project_terms"][0], "useUserQuery");
    }
}
