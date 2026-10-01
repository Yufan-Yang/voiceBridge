//! JSON-lines protocol spoken with stdio sidecars (one request line, one
//! response line). Every request carries the session token.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct SidecarRequest {
    pub id: String,
    pub token: String,
    /// `health` or `transcribe`.
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hotwords: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SidecarResponse {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub text: String,
    /// Short error category from the sidecar; never user content.
    #[serde(default)]
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_response_shapes() {
        let req = SidecarRequest {
            id: "1".into(),
            token: "t".into(),
            op: "health".into(),
            audio_path: None,
            hotwords: vec![],
            language: None,
        };
        assert_eq!(
            serde_json::to_string(&req).unwrap(),
            r#"{"id":"1","token":"t","op":"health"}"#
        );
        let resp: SidecarResponse = serde_json::from_str(r#"{"id":"1","ok":true}"#).unwrap();
        assert!(resp.ok && resp.text.is_empty());
    }
}
