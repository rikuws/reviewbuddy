use std::{path::Path, time::Duration};

pub use crate::command_runner::CommandOutput;

use crate::command_runner::CommandRunner;

pub fn run(args: &[&str]) -> Result<CommandOutput, String> {
    let owned_args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    run_owned(owned_args)
}

pub fn run_owned(args: Vec<String>) -> Result<CommandOutput, String> {
    run_owned_in(args, None::<&Path>)
}

pub fn run_owned_in<P>(
    args: Vec<String>,
    working_directory: Option<P>,
) -> Result<CommandOutput, String>
where
    P: AsRef<Path>,
{
    let mut runner = CommandRunner::new("gh")
        .args(args)
        .timeout(Duration::from_secs(120));

    if let Some(path) = working_directory {
        runner = runner.current_dir(path.as_ref());
    }

    let output = runner.run()?;
    if output.timed_out {
        return Err("gh command timed out after 120 seconds.".to_string());
    }
    Ok(output)
}

pub fn run_json_owned(args: Vec<String>) -> Result<serde_json::Value, String> {
    let output = run_owned(args)?;

    if output.exit_code != Some(0) {
        return Err(if !output.stderr.is_empty() {
            output.stderr
        } else if !output.stdout.is_empty() {
            output.stdout
        } else {
            "gh command failed with no stderr output.".to_string()
        });
    }

    serde_json::from_str(&output.stdout)
        .or_else(|_| {
            extract_json_document(&output.stdout)
                .ok_or_else(|| {
                    serde_json::Error::io(std::io::Error::other("no JSON document found"))
                })
                .and_then(serde_json::from_str)
        })
        .map_err(|error| format!("Failed to parse gh JSON output: {error}"))
}

pub fn graphql(query: &str, variables: serde_json::Value) -> Result<serde_json::Value, String> {
    let mut args = vec![
        "api".to_string(),
        "graphql".to_string(),
        "--raw-field".to_string(),
        format!("query={query}"),
    ];

    if let Some(object) = variables.as_object() {
        for (key, value) in object {
            args.push("--field".to_string());
            args.push(format!("{key}={}", graphql_variable_value(value)));
        }
    }

    run_json_owned(args)
}

fn graphql_variable_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(boolean) => boolean.to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(string) => string.clone(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn extract_json_document(output: &str) -> Option<&str> {
    let object_start = output.find('{');
    let array_start = output.find('[');

    match (object_start, array_start) {
        (Some(object_index), Some(array_index)) => {
            let start = object_index.min(array_index);
            Some(&output[start..])
        }
        (Some(start), None) | (None, Some(start)) => Some(&output[start..]),
        (None, None) => None,
    }
}
