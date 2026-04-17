use once_cell::sync::Lazy;
use serde_json::Value;

pub const TOUR_OUTPUT_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "summary": { "type": "string" },
    "reviewFocus": { "type": "string" },
    "openQuestions": {
      "type": "array",
      "items": { "type": "string" }
    },
    "warnings": {
      "type": "array",
      "items": { "type": "string" }
    },
    "overview": {
      "type": "object",
      "properties": {
        "title": { "type": "string" },
        "summary": { "type": "string" },
        "detail": { "type": "string" },
        "badge": { "type": "string" }
      },
      "required": ["title", "summary", "detail", "badge"],
      "additionalProperties": false
    },
    "steps": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "sourceStepId": { "type": "string" },
          "title": { "type": "string" },
          "summary": { "type": "string" },
          "detail": { "type": "string" },
          "badge": { "type": "string" }
        },
        "required": ["sourceStepId", "title", "summary", "detail", "badge"],
        "additionalProperties": false
      }
    },
    "sections": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "title": { "type": "string" },
          "summary": { "type": "string" },
          "detail": { "type": "string" },
          "badge": { "type": "string" },
          "stepIds": {
            "type": "array",
            "items": { "type": "string" }
          },
          "reviewPoints": {
            "type": "array",
            "items": { "type": "string" }
          },
          "callsites": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "title": { "type": "string" },
                "path": { "type": "string" },
                "line": { "type": ["integer", "null"] },
                "summary": { "type": "string" },
                "snippet": { "type": ["string", "null"] }
              },
              "required": ["title", "path", "line", "summary", "snippet"],
              "additionalProperties": false
            }
          }
        },
        "required": [
          "title",
          "summary",
          "detail",
          "badge",
          "stepIds",
          "reviewPoints",
          "callsites"
        ],
        "additionalProperties": false
      }
    }
  },
  "required": [
    "summary",
    "reviewFocus",
    "openQuestions",
    "warnings",
    "overview",
    "steps",
    "sections"
  ],
  "additionalProperties": false
}"#;

#[allow(dead_code)]
pub static TOUR_OUTPUT_SCHEMA_VALUE: Lazy<Value> = Lazy::new(|| {
    serde_json::from_str(TOUR_OUTPUT_SCHEMA_JSON)
        .expect("TOUR_OUTPUT_SCHEMA_JSON must be valid JSON")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_parses() {
        let value = &*TOUR_OUTPUT_SCHEMA_VALUE;
        assert_eq!(value["type"], "object");
        assert!(value["properties"]["overview"].is_object());
    }
}
