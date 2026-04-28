use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct JsonParseError {
    pub message: String,
}

impl std::fmt::Display for JsonParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JsonParseError {}

pub fn parse_tolerant<T: DeserializeOwned>(raw: &str) -> Result<T, JsonParseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(JsonParseError {
            message: "The response was empty.".to_string(),
        });
    }

    let unwrapped = unwrap_markdown_fence(trimmed);
    let extracted = extract_first_json_object(unwrapped);

    let mut candidates: Vec<&str> = Vec::new();
    candidates.push(trimmed);
    if unwrapped != trimmed {
        candidates.push(unwrapped);
    }
    if let Some(ref slice) = extracted {
        if !candidates.contains(&slice.as_str()) {
            candidates.push(slice);
        }
    }

    let mut last_serde_error: Option<serde_json::Error> = None;
    for candidate in &candidates {
        match serde_json::from_str::<T>(candidate) {
            Ok(value) => return Ok(value),
            Err(error) => last_serde_error = Some(error),
        }
    }

    #[cfg(feature = "tolerant-json")]
    {
        let mut last_json5_error: Option<String> = None;
        for candidate in &candidates {
            match json5::from_str::<T>(candidate) {
                Ok(value) => return Ok(value),
                Err(error) => last_json5_error = Some(error.to_string()),
            }
        }

        Err(JsonParseError {
            message: format!(
                "Failed to parse the LLM response as JSON: {}{}",
                last_serde_error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "unknown error".to_string()),
                last_json5_error
                    .map(|error| format!(" (json5 repair also failed: {error})"))
                    .unwrap_or_default(),
            ),
        })
    }

    #[cfg(not(feature = "tolerant-json"))]
    {
        Err(JsonParseError {
            message: format!(
                "Failed to parse the LLM response as JSON: {}",
                last_serde_error
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "unknown error".to_string()),
            ),
        })
    }
}

fn unwrap_markdown_fence(value: &str) -> &str {
    let trimmed = value.trim();
    if !trimmed.starts_with("```") {
        return trimmed;
    }

    let after_opener = trimmed.trim_start_matches('`');
    let after_opener = after_opener
        .trim_start_matches("json")
        .trim_start_matches('\n');
    if let Some(end) = after_opener.rfind("```") {
        after_opener[..end].trim()
    } else {
        trimmed
    }
}

fn extract_first_json_object(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let start = bytes.iter().position(|byte| *byte == b'{')?;

    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate().skip(start) {
        let character = *byte;

        if in_string {
            if escaped {
                escaped = false;
            } else if character == b'\\' {
                escaped = true;
            } else if character == b'"' {
                in_string = false;
            }
            continue;
        }

        match character {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(value[start..=index].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn parses_plain_json() {
        let value: Value = parse_tolerant("{\"foo\": 1}").expect("parse");
        assert_eq!(value["foo"], 1);
    }

    #[test]
    fn unwraps_markdown_fences() {
        let raw = "```json\n{\"foo\": 2}\n```";
        let value: Value = parse_tolerant(raw).expect("parse");
        assert_eq!(value["foo"], 2);
    }

    #[test]
    fn extracts_embedded_object() {
        let raw = "sure, here is the tour: {\"foo\": 3} — done!";
        let value: Value = parse_tolerant(raw).expect("parse");
        assert_eq!(value["foo"], 3);
    }

    #[cfg(feature = "tolerant-json")]
    #[test]
    fn repairs_trailing_commas() {
        let raw = "{\"foo\": 4, \"bar\": [1, 2, 3,],}";
        let value: Value = parse_tolerant(raw).expect("parse");
        assert_eq!(value["foo"], 4);
        assert_eq!(value["bar"][2], 3);
    }
}
