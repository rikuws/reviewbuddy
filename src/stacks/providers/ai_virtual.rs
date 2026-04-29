use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};

use crate::{
    agents::{
        self,
        jsonrepair::parse_tolerant,
        prompt::{build_stack_planning_prompt, build_stack_planning_refinement_prompt},
    },
    app_storage,
    code_tour::CodeTourProvider,
    github::PullRequestDetail,
};

use super::super::{
    atoms::extract_change_atoms,
    dependencies::{build_atom_dependencies, dependency_depths, AtomDependency, DependencyKind},
    model::{
        stack_now_ms, ChangeAtom, ChangeAtomId, ChangeAtomSource, ChangeRole, Confidence,
        LayerMetrics, LayerReviewStatus, RepoContext, ReviewStack, ReviewStackLayer,
        StackDiscoveryError, StackKind, StackProviderMetadata, StackSource, StackWarning,
        VirtualLayerRef, VirtualStackSizing, STACK_GENERATOR_VERSION,
    },
    validation::{
        atom_is_substantive, atom_noise_kind, requires_manual_review, validate_ai_stack_plan,
        AiReviewPriority, AiStackPlan, AiStackPlanLayer, ValidatedAiStackPlan,
    },
};

use super::virtual_commits::{self, CommitContext, CommitSuitability, CommitSummary};

const MAX_AI_STACK_ATOMS: usize = 180;
const MAX_AI_STACK_ATTEMPTS: usize = 5;

pub struct AiVirtualStackProvider;

pub fn discover(
    selected_pr: &PullRequestDetail,
    repo_context: &RepoContext,
    _sizing: &VirtualStackSizing,
    provider: CodeTourProvider,
) -> Result<Option<ReviewStack>, StackDiscoveryError> {
    let atoms = extract_change_atoms(selected_pr);
    if atoms.is_empty() {
        return Ok(None);
    }

    let commit_context = virtual_commits::commit_context_for_pr(selected_pr, repo_context, &atoms)?
        .unwrap_or_else(|| missing_commit_context(selected_pr.commits_count));
    let total_changed_lines = atoms
        .iter()
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>()
        .max((selected_pr.additions + selected_pr.deletions).max(0) as usize);

    if !should_attempt_ai_planning(&atoms, total_changed_lines, &commit_context.suitability) {
        return Ok(None);
    }

    if atoms.len() > MAX_AI_STACK_ATOMS {
        return ai_unavailable_stack(
            selected_pr,
            "AI stack planning was unavailable because the atom list exceeded the prompt budget. Remiss did not generate a non-AI stack.",
            Some(json!({
                "atomCount": atoms.len(),
                "maxAiStackAtoms": MAX_AI_STACK_ATOMS,
                "commitSuitability": commit_context.suitability,
            })),
        )
        .map(Some);
    }

    let Some(working_directory) = repo_context.local_repo_path.as_ref() else {
        return ai_unavailable_stack(
            selected_pr,
            "AI stack planning was unavailable because no local checkout was ready. Remiss did not generate a non-AI stack.",
            Some(json!({ "commitSuitability": commit_context.suitability })),
        )
        .map(Some);
    };

    let backend = agents::backend_for(provider);
    match backend.status() {
        Ok(status) if status.available && status.authenticated => {}
        Ok(status) => {
            return ai_unavailable_stack(
                selected_pr,
                "AI stack planning was unavailable. Remiss did not generate a non-AI stack.",
                Some(json!({
                    "provider": provider.slug(),
                    "status": status,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
        Err(error) => {
            return ai_unavailable_stack(
                selected_pr,
                "AI stack planning was unavailable. Remiss did not generate a non-AI stack.",
                Some(json!({
                    "provider": provider.slug(),
                    "error": error,
                    "commitSuitability": commit_context.suitability,
                })),
            )
            .map(Some);
        }
    }

    let input_json = build_stack_planning_input(selected_pr, &atoms, &commit_context);
    let initial_prompt = build_stack_planning_prompt(&input_json);

    let mut prompt = initial_prompt.clone();
    let mut attempt: usize = 0;
    let mut prior_failures: Vec<Value> = Vec::new();

    loop {
        attempt += 1;
        let response = match agents::run_json_prompt(
            provider,
            working_directory.to_string_lossy().as_ref(),
            prompt.clone(),
        ) {
            Ok(response) => response,
            Err(error) => {
                let diagnostic_log_path =
                    write_ai_stack_diagnostic_log(AiStackDiagnosticLogInput {
                        selected_pr,
                        provider,
                        stage: "provider_error",
                        working_directory: working_directory.to_string_lossy().as_ref(),
                        input_json: &input_json,
                        prompt: &prompt,
                        response_text: None,
                        model: None,
                        parse_error: None,
                        validation_error: None,
                        parsed_plan: None,
                        extracted_plan_value: None,
                        normalized_plan_value: None,
                        provider_error: Some(&error),
                        attempt,
                        max_attempts: MAX_AI_STACK_ATTEMPTS,
                        prior_failures: &prior_failures,
                    });
                return ai_unavailable_stack(
                    selected_pr,
                    &format!(
                        "AI stack planning was unavailable.{} Remiss did not generate a non-AI stack.",
                        diagnostic_log_suffix(diagnostic_log_path.as_deref())
                    ),
                    Some(json!({
                        "provider": provider.slug(),
                        "error": error,
                        "diagnosticLogPath": diagnostic_log_path,
                        "commitSuitability": commit_context.suitability,
                        "attempts": attempt,
                    })),
                )
                .map(Some);
            }
        };

        let parse_result = match parse_ai_stack_plan_response(&response.text) {
            Ok(result) => result,
            Err(error) => {
                if attempt < MAX_AI_STACK_ATTEMPTS {
                    prior_failures.push(json!({
                        "attempt": attempt,
                        "stage": "parse_error",
                        "message": error.message,
                        "model": response.model,
                    }));
                    prompt = build_stack_planning_refinement_prompt(
                        &input_json,
                        &response.text,
                        "Parse error",
                        &error.message,
                        attempt + 1,
                        MAX_AI_STACK_ATTEMPTS,
                    );
                    continue;
                }
                let diagnostic_log_path =
                    write_ai_stack_diagnostic_log(AiStackDiagnosticLogInput {
                        selected_pr,
                        provider,
                        stage: "parse_error",
                        working_directory: working_directory.to_string_lossy().as_ref(),
                        input_json: &input_json,
                        prompt: &prompt,
                        response_text: Some(&response.text),
                        model: response.model.as_deref(),
                        parse_error: Some(&error),
                        validation_error: None,
                        parsed_plan: None,
                        extracted_plan_value: error.extracted_plan_value.as_ref(),
                        normalized_plan_value: error.normalized_plan_value.as_ref(),
                        provider_error: None,
                        attempt,
                        max_attempts: MAX_AI_STACK_ATTEMPTS,
                        prior_failures: &prior_failures,
                    });
                return ai_unavailable_stack(
                    selected_pr,
                    &format!(
                        "AI stack planning returned invalid output after {} attempts: {}.{} Remiss did not generate a non-AI stack.",
                        attempt,
                        error.message,
                        diagnostic_log_suffix(diagnostic_log_path.as_deref())
                    ),
                    Some(json!({
                        "provider": provider.slug(),
                        "modelOrAgent": response.model,
                        "error": error.message,
                        "diagnosticLogPath": diagnostic_log_path,
                        "commitSuitability": commit_context.suitability,
                        "attempts": attempt,
                    })),
                )
                .map(Some);
            }
        };
        let plan = parse_result.plan.clone();

        match validate_ai_stack_plan(&plan, &atoms, total_changed_lines) {
            Ok(validated) => {
                return Ok(Some(build_stack_from_validated_plan(
                    selected_pr,
                    atoms,
                    validated,
                    response.model,
                    provider,
                    commit_context,
                )));
            }
            Err(error) => {
                if attempt < MAX_AI_STACK_ATTEMPTS {
                    prior_failures.push(json!({
                        "attempt": attempt,
                        "stage": "validation_error",
                        "message": error.message,
                        "model": response.model,
                    }));
                    prompt = build_stack_planning_refinement_prompt(
                        &input_json,
                        &response.text,
                        "Validation error",
                        &error.message,
                        attempt + 1,
                        MAX_AI_STACK_ATTEMPTS,
                    );
                    continue;
                }
                let diagnostic_log_path =
                    write_ai_stack_diagnostic_log(AiStackDiagnosticLogInput {
                        selected_pr,
                        provider,
                        stage: "validation_error",
                        working_directory: working_directory.to_string_lossy().as_ref(),
                        input_json: &input_json,
                        prompt: &prompt,
                        response_text: Some(&response.text),
                        model: response.model.as_deref(),
                        parse_error: None,
                        validation_error: Some(&error.message),
                        parsed_plan: Some(&plan),
                        extracted_plan_value: Some(&parse_result.extracted_plan_value),
                        normalized_plan_value: Some(&parse_result.normalized_plan_value),
                        provider_error: None,
                        attempt,
                        max_attempts: MAX_AI_STACK_ATTEMPTS,
                        prior_failures: &prior_failures,
                    });
                return ai_unavailable_stack(
                    selected_pr,
                    &format!(
                        "AI stack planning returned invalid output after {} attempts: {}.{} Remiss did not generate a non-AI stack.",
                        attempt,
                        error.message,
                        diagnostic_log_suffix(diagnostic_log_path.as_deref())
                    ),
                    Some(json!({
                        "provider": provider.slug(),
                        "modelOrAgent": response.model,
                        "validationError": error.message,
                        "diagnosticLogPath": diagnostic_log_path,
                        "commitSuitability": commit_context.suitability,
                        "attempts": attempt,
                    })),
                )
                .map(Some);
            }
        }
    }
}

pub fn should_attempt_ai_planning(
    atoms: &[ChangeAtom],
    _total_changed_lines: usize,
    _commit_suitability: &CommitSuitability,
) -> bool {
    !atoms.is_empty()
}

#[derive(Debug)]
struct AiStackPlanParseResult {
    plan: AiStackPlan,
    extracted_plan_value: Value,
    normalized_plan_value: Value,
}

#[derive(Debug)]
struct AiStackPlanParseError {
    message: String,
    extracted_plan_value: Option<Value>,
    normalized_plan_value: Option<Value>,
}

fn parse_ai_stack_plan_response(
    raw: &str,
) -> Result<AiStackPlanParseResult, AiStackPlanParseError> {
    let value = parse_tolerant::<Value>(raw).map_err(|error| AiStackPlanParseError {
        message: error.message,
        extracted_plan_value: None,
        normalized_plan_value: None,
    })?;
    let mut plan_value =
        extract_ai_stack_plan_value(value).ok_or_else(|| AiStackPlanParseError {
            message: "The response did not contain a stack plan object with layers.".to_string(),
            extracted_plan_value: None,
            normalized_plan_value: None,
        })?;

    let extracted_plan_value = plan_value.clone();
    normalize_ai_stack_plan_value(&mut plan_value);
    let normalized_plan_value = plan_value.clone();
    match serde_json::from_value::<AiStackPlan>(plan_value) {
        Ok(plan) => Ok(AiStackPlanParseResult {
            plan,
            extracted_plan_value,
            normalized_plan_value,
        }),
        Err(error) => Err(AiStackPlanParseError {
            message: format!("The stack plan did not match the expected schema: {error}"),
            extracted_plan_value: Some(extracted_plan_value),
            normalized_plan_value: Some(normalized_plan_value),
        }),
    }
}

fn extract_ai_stack_plan_value(value: Value) -> Option<Value> {
    if value_has_layers(&value) {
        return Some(value);
    }

    let object = value.as_object()?;
    for key in [
        "plan",
        "stack_plan",
        "stackPlan",
        "review_stack_plan",
        "reviewStackPlan",
        "review_stack",
        "reviewStack",
        "result",
        "response",
        "output",
        "data",
    ] {
        let Some(candidate) = object.get(key) else {
            continue;
        };
        if value_has_layers(candidate) {
            return Some(candidate.clone());
        }
        if let Some(text) = candidate.as_str() {
            if let Ok(parsed) = parse_tolerant::<Value>(text) {
                if value_has_layers(&parsed) {
                    return Some(parsed);
                }
            }
        }
    }

    None
}

fn value_has_layers(value: &Value) -> bool {
    value
        .as_object()
        .map(|object| {
            object.contains_key("layers")
                || object.contains_key("Layers")
                || object.contains_key("reviewLayers")
                || object.contains_key("review_layers")
                || object.contains_key("stackLayers")
                || object.contains_key("stack_layers")
        })
        .unwrap_or(false)
}

fn normalize_ai_stack_plan_value(value: &mut Value) {
    normalize_object_keys_to_snake_case(value);
    let Some(object) = value.as_object_mut() else {
        return;
    };

    alias_field(object, "review_layers", "layers");
    alias_field(object, "stack_layers", "layers");
    object
        .entry("strategy")
        .or_insert_with(|| Value::String("semantic_virtual_stack".to_string()));
    object
        .entry("confidence")
        .or_insert_with(|| Value::String("medium".to_string()));
    object.entry("rationale").or_insert_with(|| {
        Value::String("AI grouped pull request atoms into review layers.".to_string())
    });
    object
        .entry("manual_review_atom_ids")
        .or_insert_with(|| Value::Array(Vec::new()));
    object
        .entry("warnings")
        .or_insert_with(|| Value::Array(Vec::new()));

    normalize_enum_field(object, "strategy");
    normalize_enum_field(object, "confidence");
    normalize_atom_id_array_field(object, "manual_review_atom_ids");

    if let Some(layers) = object.get_mut("layers").and_then(Value::as_array_mut) {
        for (index, layer) in layers.iter_mut().enumerate() {
            normalize_ai_stack_layer_value(layer, index);
        }
    }
}

fn normalize_ai_stack_layer_value(value: &mut Value, index: usize) {
    normalize_object_keys_to_snake_case(value);
    let Some(object) = value.as_object_mut() else {
        return;
    };

    alias_field(object, "depends_on", "depends_on_layer_indexes");
    alias_field(object, "dependencies", "depends_on_layer_indexes");
    alias_field(object, "atoms", "atom_ids");
    object
        .entry("title")
        .or_insert_with(|| Value::String(format!("Review layer {}", index + 1)));
    object.entry("review_question").or_insert_with(|| {
        Value::String("Does this layer answer one coherent review question?".to_string())
    });
    object
        .entry("summary")
        .or_insert_with(|| Value::String("Review this coherent layer of changes.".to_string()));
    object
        .entry("rationale")
        .or_insert_with(|| Value::String("Grouped together by the AI stack planner.".to_string()));
    object
        .entry("substantive_atom_ids")
        .or_insert_with(|| Value::Array(Vec::new()));
    object
        .entry("attached_noise_atom_ids")
        .or_insert_with(|| Value::Array(Vec::new()));
    object
        .entry("atom_ids")
        .or_insert_with(|| Value::Array(Vec::new()));
    object
        .entry("depends_on_layer_indexes")
        .or_insert_with(|| Value::Array(Vec::new()));
    object
        .entry("confidence")
        .or_insert_with(|| Value::String("medium".to_string()));
    object
        .entry("review_priority")
        .or_insert_with(|| Value::String("normal".to_string()));

    normalize_enum_field(object, "confidence");
    normalize_enum_field(object, "review_priority");
    normalize_atom_id_array_field(object, "substantive_atom_ids");
    normalize_atom_id_array_field(object, "attached_noise_atom_ids");
    normalize_atom_id_array_field(object, "atom_ids");
}

fn normalize_object_keys_to_snake_case(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let previous = std::mem::take(object);
            for (key, mut nested) in previous {
                normalize_object_keys_to_snake_case(&mut nested);
                object.insert(to_snake_case_key(&key), nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_object_keys_to_snake_case(item);
            }
        }
        _ => {}
    }
}

fn alias_field(object: &mut Map<String, Value>, from: &str, to: &str) {
    if object.contains_key(to) {
        return;
    }
    if let Some(value) = object.remove(from) {
        object.insert(to.to_string(), value);
    }
}

fn normalize_enum_field(object: &mut Map<String, Value>, key: &str) {
    let Some(value) = object.get_mut(key) else {
        return;
    };
    if let Some(text) = value.as_str() {
        *value = Value::String(normalize_enum_token(text));
    }
}

fn normalize_atom_id_array_field(object: &mut Map<String, Value>, key: &str) {
    let Some(value) = object.get_mut(key) else {
        return;
    };
    let Some(items) = value.as_array_mut() else {
        return;
    };

    for item in items {
        if item.is_string() {
            continue;
        }
        if let Some(id) = item
            .as_object()
            .and_then(|object| object.get("id").or_else(|| object.get("atom_id")))
            .and_then(Value::as_str)
        {
            *item = Value::String(id.to_string());
        }
    }
}

fn to_snake_case_key(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_separator = false;
    for (index, character) in value.chars().enumerate() {
        if matches!(character, '-' | ' ' | '.') {
            if !output.is_empty() && !previous_was_separator {
                output.push('_');
                previous_was_separator = true;
            }
            continue;
        }

        if character.is_ascii_uppercase() {
            if index > 0 && !previous_was_separator {
                output.push('_');
            }
            output.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else {
            output.push(character);
            previous_was_separator = character == '_';
        }
    }
    output.trim_matches('_').to_string()
}

fn normalize_enum_token(value: &str) -> String {
    to_snake_case_key(value.trim().trim_matches('"').trim())
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

struct AiStackDiagnosticLogInput<'a> {
    selected_pr: &'a PullRequestDetail,
    provider: CodeTourProvider,
    stage: &'a str,
    working_directory: &'a str,
    input_json: &'a Value,
    prompt: &'a str,
    response_text: Option<&'a str>,
    model: Option<&'a str>,
    parse_error: Option<&'a AiStackPlanParseError>,
    validation_error: Option<&'a str>,
    parsed_plan: Option<&'a AiStackPlan>,
    extracted_plan_value: Option<&'a Value>,
    normalized_plan_value: Option<&'a Value>,
    provider_error: Option<&'a str>,
    attempt: usize,
    max_attempts: usize,
    prior_failures: &'a [Value],
}

fn write_ai_stack_diagnostic_log(input: AiStackDiagnosticLogInput<'_>) -> Option<String> {
    let timestamp_ms = stack_now_ms();
    let log_dir = app_storage::data_dir_root().join("ai-stack-logs");
    if let Err(error) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Failed to create AI stack diagnostic log directory '{}': {error}",
            log_dir.display()
        );
        return None;
    }

    let file_name = format!(
        "{}-{}-pr-{}-{}.json",
        timestamp_ms,
        sanitize_log_path_component(&input.selected_pr.repository),
        input.selected_pr.number,
        input.provider.slug()
    );
    let path = log_dir.join(file_name);
    let parse_error = input.parse_error.map(|error| {
        json!({
            "message": error.message,
            "extractedPlanAvailable": error.extracted_plan_value.is_some(),
            "normalizedPlanAvailable": error.normalized_plan_value.is_some(),
        })
    });
    let log = json!({
        "timestampMs": timestamp_ms,
        "stage": input.stage,
        "repository": input.selected_pr.repository,
        "prNumber": input.selected_pr.number,
        "headRefOid": input.selected_pr.head_ref_oid,
        "provider": input.provider.slug(),
        "workingDirectory": input.working_directory,
        "modelOrAgent": input.model,
        "providerError": input.provider_error,
        "parseError": parse_error,
        "validationError": input.validation_error,
        "attempt": input.attempt,
        "maxAttempts": input.max_attempts,
        "priorFailures": input.prior_failures,
        "promptBytes": input.prompt.len(),
        "rawResponseBytes": input.response_text.map(str::len),
        "input": input.input_json,
        "prompt": input.prompt,
        "rawResponse": input.response_text,
        "extractedPlan": input.extracted_plan_value,
        "normalizedPlan": input.normalized_plan_value,
        "parsedPlan": input.parsed_plan,
    });

    let serialized = match serde_json::to_string_pretty(&log) {
        Ok(serialized) => serialized,
        Err(error) => {
            eprintln!("Failed to serialize AI stack diagnostic log: {error}");
            return None;
        }
    };

    if let Err(error) = std::fs::write(&path, serialized) {
        eprintln!(
            "Failed to write AI stack diagnostic log '{}': {error}",
            path.display()
        );
        return None;
    }

    let path = path.display().to_string();
    eprintln!("AI stack diagnostic log written: {path}");
    Some(path)
}

fn diagnostic_log_suffix(path: Option<&str>) -> String {
    path.map(|path| format!(" Diagnostic log: {path}."))
        .unwrap_or_default()
}

fn sanitize_log_path_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('_');
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized.chars().take(80).collect()
    }
}

pub fn build_stack_from_validated_plan(
    selected_pr: &PullRequestDetail,
    atoms: Vec<ChangeAtom>,
    validated: ValidatedAiStackPlan,
    model_or_agent: Option<String>,
    provider: CodeTourProvider,
    commit_context: CommitContext,
) -> ReviewStack {
    let plan = validated.plan;
    let stack_id = virtual_stack_id(selected_pr);
    let atoms_by_id = atoms
        .iter()
        .map(|atom| (atom.id.clone(), atom))
        .collect::<BTreeMap<_, _>>();
    let mut layers = Vec::<ReviewStackLayer>::new();

    for (index, plan_layer) in plan.layers.iter().enumerate() {
        let layer_atoms = atom_refs_for_ids(&plan_layer.atom_ids, &atoms_by_id);
        let role = dominant_role(&layer_atoms);
        let metrics = metrics_for_atoms(&layer_atoms);
        let layer_id = virtual_layer_id(&stack_id, index, role, &plan_layer.atom_ids);
        let warnings = layer_atoms
            .iter()
            .flat_map(|atom| atom.warnings.iter().cloned())
            .collect::<Vec<_>>();

        layers.push(ReviewStackLayer {
            id: layer_id,
            index,
            title: clean_layer_text(&plan_layer.title, "AI review layer", 90),
            summary: clean_layer_text(
                &layer_summary_with_review_question(plan_layer),
                "AI grouped review layer.",
                260,
            ),
            rationale: clean_layer_text(
                &layer_rationale_with_review_question(plan_layer),
                "AI grouped these changes by semantic review order.",
                560,
            ),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualAi,
                role,
                source_label: review_priority_label(&plan_layer.review_priority).to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids: plan_layer.atom_ids.clone(),
            depends_on_layer_ids: Vec::new(),
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: plan_layer.confidence,
            warnings,
        });
    }

    let layer_ids = layers
        .iter()
        .map(|layer| layer.id.clone())
        .collect::<Vec<_>>();
    for (index, plan_layer) in plan.layers.iter().enumerate() {
        layers[index].depends_on_layer_ids = plan_layer
            .depends_on_layer_indexes
            .iter()
            .filter_map(|dep_index| layer_ids.get(*dep_index).cloned())
            .collect();
    }

    if !plan.manual_review_atom_ids.is_empty() {
        let index = layers.len();
        let manual_atoms = atom_refs_for_ids(&plan.manual_review_atom_ids, &atoms_by_id);
        let metrics = metrics_for_atoms(&manual_atoms);
        let role = dominant_role(&manual_atoms);
        let layer_id = virtual_layer_id(&stack_id, index, role, &plan.manual_review_atom_ids);
        let warnings = manual_atoms
            .iter()
            .flat_map(|atom| atom.warnings.iter().cloned())
            .chain(std::iter::once(StackWarning::new(
                "manual-review",
                "AI marked these atoms for manual review.",
            )))
            .collect::<Vec<_>>();
        layers.push(ReviewStackLayer {
            id: layer_id,
            index,
            title: "Manual review / uncertain changes".to_string(),
            summary: format!(
                "{} atom{} need a whole-file or manual pass.",
                plan.manual_review_atom_ids.len(),
                if plan.manual_review_atom_ids.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            rationale: "AI marked these atoms as generated, binary, huge, ambiguous, or low-confidence. Remiss keeps them visible as an explicit final layer."
                .to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualAi,
                role,
                source_label: "manual_review".to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids: plan.manual_review_atom_ids.clone(),
            depends_on_layer_ids: layers
                .last()
                .map(|layer| vec![layer.id.clone()])
                .unwrap_or_default(),
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings,
        });
    }

    let mut warnings = plan
        .warnings
        .iter()
        .map(|warning| StackWarning::new("ai-stack-warning", warning.clone()))
        .collect::<Vec<_>>();
    warnings.extend(
        atoms
            .iter()
            .filter(|atom| requires_manual_review(atom))
            .flat_map(|atom| atom.warnings.iter().cloned()),
    );

    ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualAi,
        kind: StackKind::Virtual,
        confidence: plan.confidence,
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers,
        atoms,
        warnings,
        provider: Some(StackProviderMetadata {
            provider: "ai_virtual_stack".to_string(),
            raw_payload: Some(json!({
                "modelOrAgent": model_or_agent.unwrap_or_else(|| provider.label().to_string()),
                "strategy": plan.strategy,
                "rationale": plan.rationale,
                "commitSuitability": commit_context.suitability,
                "commits": commit_context.commits,
                "aiOnly": true,
            })),
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    }
}

pub fn ai_unavailable_stack(
    selected_pr: &PullRequestDetail,
    warning: &str,
    raw_payload: Option<Value>,
) -> Result<ReviewStack, StackDiscoveryError> {
    let atoms = extract_change_atoms(selected_pr);
    let atoms_by_ref = atoms.iter().collect::<Vec<_>>();
    let atom_ids = atoms.iter().map(|atom| atom.id.clone()).collect::<Vec<_>>();
    let stack_id = virtual_stack_id(selected_pr);
    let metrics = metrics_for_atoms(&atoms_by_ref);
    let layer_id = virtual_layer_id(&stack_id, 0, ChangeRole::Unknown, &atom_ids);
    let warning = StackWarning::new("ai-virtual-stack-unavailable", warning.to_string());

    Ok(ReviewStack {
        id: stack_id,
        repository: selected_pr.repository.clone(),
        selected_pr_number: selected_pr.number,
        source: StackSource::VirtualAi,
        kind: StackKind::Virtual,
        confidence: Confidence::Low,
        trunk_branch: Some(selected_pr.base_ref_name.clone()),
        base_oid: selected_pr.base_ref_oid.clone(),
        head_oid: selected_pr.head_ref_oid.clone(),
        layers: vec![ReviewStackLayer {
            id: layer_id,
            index: 0,
            title: "AI stack unavailable".to_string(),
            summary: "Remiss did not generate a non-AI stack for this pull request.".to_string(),
            rationale: "The stacked review view is configured to require AI-generated layers. Review the whole PR while the selected AI provider is unavailable or returns invalid output.".to_string(),
            pr: None,
            virtual_layer: Some(VirtualLayerRef {
                source: StackSource::VirtualAi,
                role: ChangeRole::Unknown,
                source_label: "ai_unavailable".to_string(),
            }),
            base_oid: selected_pr.base_ref_oid.clone(),
            head_oid: selected_pr.head_ref_oid.clone(),
            atom_ids,
            depends_on_layer_ids: Vec::new(),
            metrics,
            status: LayerReviewStatus::NotReviewed,
            confidence: Confidence::Low,
            warnings: vec![warning.clone()],
        }],
        atoms,
        warnings: vec![warning],
        provider: Some(StackProviderMetadata {
            provider: "ai_virtual_stack".to_string(),
            raw_payload: Some(json!({
                "aiOnly": true,
                "aiVirtualStack": raw_payload.unwrap_or(Value::Null),
            })),
        }),
        generated_at_ms: stack_now_ms(),
        generator_version: STACK_GENERATOR_VERSION.to_string(),
    })
}

fn build_stack_planning_input(
    selected_pr: &PullRequestDetail,
    atoms: &[ChangeAtom],
    commit_context: &CommitContext,
) -> Value {
    let commits_by_path = commits_by_path(&commit_context.commits);
    let dependencies = build_atom_dependencies(atoms);
    let atom_ids = atoms.iter().map(|atom| atom.id.clone()).collect::<Vec<_>>();
    let depths = dependency_depths(&atom_ids, &dependencies);
    let total_changed_lines = atoms
        .iter()
        .map(|atom| atom.additions + atom.deletions)
        .sum::<usize>();

    json!({
        "repository": &selected_pr.repository,
        "pr_number": selected_pr.number,
        "title": &selected_pr.title,
        "body_summary": crate::agents::prompt::trim_text(&selected_pr.body, 1_200),
        "total_files": selected_pr.changed_files,
        "total_changed_lines": total_changed_lines,
        "commit_suitability": &commit_context.suitability,
        "commits": commit_context
            .commits
            .iter()
            .map(commit_summary_json)
            .collect::<Vec<_>>(),
        "dependency_edges": dependencies
            .iter()
            .filter(|dep| matches!(
                dep.kind,
                DependencyKind::SymbolReference | DependencyKind::TestTarget
            ))
            .map(dependency_json)
            .collect::<Vec<_>>(),
        "candidate_layers": candidate_layers_json(atoms, &depths),
        "hard_validation_rules": [
            "Every layer needs at least one substantive atom unless it is a coherent mechanical formatting/comment layer.",
            "Import-only and mostly import layers are invalid; attach import atoms to the substantive atom that requires them.",
            "The final layer must not contain more than 40% of substantive atoms or more than two concerns.",
            "Tests usually travel with the behavior they validate; generic tests-only layers are invalid.",
            "Dependency edges must point from lower layers to same or higher layers.",
            "Misc, remaining, update imports, cleanup, and everything else titles are invalid.",
            "Prefer fewer coherent layers over artificial layers."
        ],
        "atoms": atoms
            .iter()
            .map(|atom| atom_summary_json(atom, &commits_by_path, &depths))
            .collect::<Vec<_>>(),
    })
}

fn dependency_json(dependency: &AtomDependency) -> Value {
    json!({
        "from_atom_id": &dependency.from_atom_id,
        "to_atom_id": &dependency.to_atom_id,
        "kind": &dependency.kind,
        "confidence": dependency.confidence,
    })
}

fn candidate_layers_json(
    atoms: &[ChangeAtom],
    depths: &BTreeMap<ChangeAtomId, usize>,
) -> Vec<Value> {
    let mut groups = BTreeMap::<CandidateLayerKey, CandidateLayer>::new();
    let mut substantive_key_by_path = BTreeMap::<String, CandidateLayerKey>::new();

    for atom in atoms.iter().filter(|atom| atom_is_substantive(atom)) {
        let key = CandidateLayerKey {
            role_order: atom.role.order(),
            role: atom.role,
            depth: depths.get(&atom.id).copied().unwrap_or_default(),
            directory: directory_label(atom.path.as_str()),
        };
        substantive_key_by_path
            .entry(atom.path.clone())
            .or_insert_with(|| key.clone());
        let group = groups.entry(key.clone()).or_insert_with(|| CandidateLayer {
            key,
            substantive_atom_ids: Vec::new(),
            attached_noise_atom_ids: Vec::new(),
        });
        group.substantive_atom_ids.push(atom.id.clone());
    }

    for atom in atoms.iter().filter(|atom| !atom_is_substantive(atom)) {
        let key = substantive_key_by_path
            .get(&atom.path)
            .cloned()
            .unwrap_or(CandidateLayerKey {
                role_order: atom.role.order(),
                role: atom.role,
                depth: depths.get(&atom.id).copied().unwrap_or_default(),
                directory: directory_label(atom.path.as_str()),
            });
        let group = groups.entry(key.clone()).or_insert_with(|| CandidateLayer {
            key,
            substantive_atom_ids: Vec::new(),
            attached_noise_atom_ids: Vec::new(),
        });
        group.attached_noise_atom_ids.push(atom.id.clone());
    }

    groups
        .into_values()
        .enumerate()
        .map(|(index, group)| {
            json!({
                "index": index,
                "title": candidate_layer_title(group.key.role, group.key.depth, group.key.directory.as_str()),
                "review_question": candidate_layer_question(group.key.role),
                "role": group.key.role.label(),
                "dependency_depth": group.key.depth,
                "directory": group.key.directory,
                "substantive_atom_ids": group.substantive_atom_ids,
                "attached_noise_atom_ids": group.attached_noise_atom_ids,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateLayerKey {
    role_order: usize,
    role: ChangeRole,
    depth: usize,
    directory: String,
}

struct CandidateLayer {
    key: CandidateLayerKey,
    substantive_atom_ids: Vec<ChangeAtomId>,
    attached_noise_atom_ids: Vec<ChangeAtomId>,
}

fn candidate_layer_title(role: ChangeRole, depth: usize, directory: &str) -> String {
    let verb = match role {
        ChangeRole::Foundation | ChangeRole::Config => "Introduce",
        ChangeRole::CoreLogic => "Build",
        ChangeRole::Integration => "Wire",
        ChangeRole::Presentation => "Render",
        ChangeRole::Tests => "Validate",
        ChangeRole::Docs => "Document",
        ChangeRole::Generated => "Regenerate",
        ChangeRole::Unknown => "Review",
    };

    format!(
        "{verb} {} changes in {directory} at dependency depth {depth}.",
        role.label().to_ascii_lowercase()
    )
}

fn candidate_layer_question(role: ChangeRole) -> &'static str {
    match role {
        ChangeRole::Foundation | ChangeRole::Config => {
            "Do these foundation changes establish the contract later layers depend on?"
        }
        ChangeRole::CoreLogic => {
            "Does this layer implement one coherent behavior change correctly?"
        }
        ChangeRole::Integration => "Does this layer wire the behavior through the right boundary?",
        ChangeRole::Presentation => {
            "Does this layer expose the behavior correctly in the reviewable UI surface?"
        }
        ChangeRole::Tests => {
            "Does this layer provide broad coverage that cannot travel with one behavior layer?"
        }
        ChangeRole::Docs => "Does this layer document the reviewed behavior accurately?",
        ChangeRole::Generated => "Was this mechanical generated change produced coherently?",
        ChangeRole::Unknown => "Why does this atom need manual review instead of a normal layer?",
    }
}

fn directory_label(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .filter(|dir| !dir.is_empty())
        .unwrap_or_else(|| "root".to_string())
}

fn commit_summary_json(commit: &CommitSummary) -> Value {
    json!({
        "oid": &commit.oid,
        "message": &commit.subject,
        "changed_lines": commit.changed_lines,
        "roles": commit.roles.iter().map(ChangeRole::label).collect::<Vec<_>>(),
    })
}

fn atom_summary_json(
    atom: &ChangeAtom,
    commits_by_path: &BTreeMap<String, Vec<&CommitSummary>>,
    depths: &BTreeMap<ChangeAtomId, usize>,
) -> Value {
    let commits = commits_by_path.get(&atom.path).cloned().unwrap_or_default();
    let commit_messages = commits
        .iter()
        .map(|commit| commit.subject.clone())
        .collect::<Vec<_>>();
    let mut object = serde_json::Map::new();
    object.insert("id".into(), json!(&atom.id));
    object.insert("path".into(), json!(&atom.path));
    if atom
        .previous_path
        .as_ref()
        .map(|previous| previous != &atom.path)
        .unwrap_or(false)
    {
        object.insert("previous_path".into(), json!(&atom.previous_path));
    }
    object.insert("source_kind".into(), json!(atom.source.stable_kind()));
    object.insert("role".into(), json!(atom.role.label()));
    if let Some(kind) = &atom.semantic_kind {
        object.insert("semantic_kind".into(), json!(kind));
    }
    object.insert("substantive".into(), json!(atom_is_substantive(atom)));
    if let Some(noise) = atom_noise_kind(atom) {
        object.insert("noise_kind".into(), json!(noise));
    }
    object.insert(
        "dependency_depth".into(),
        json!(depths.get(&atom.id).copied().unwrap_or_default()),
    );
    object.insert("title".into(), json!(atom_title(atom)));
    object.insert("summary".into(), json!(atom_summary(atom)));
    if let Some(symbol) = &atom.symbol_name {
        object.insert("symbol_name".into(), json!(symbol));
    }
    if !atom.defined_symbols.is_empty() {
        object.insert("defined_symbols".into(), json!(&atom.defined_symbols));
    }
    if !atom.referenced_symbols.is_empty() {
        object.insert(
            "referenced_symbols".into(),
            json!(atom.referenced_symbols.iter().take(12).collect::<Vec<_>>()),
        );
    }
    if !atom.hunk_headers.is_empty() {
        object.insert(
            "hunk_headers".into(),
            json!(atom.hunk_headers.iter().take(2).collect::<Vec<_>>()),
        );
    }
    object.insert(
        "changed_line_count".into(),
        json!(atom.additions + atom.deletions),
    );
    if !commit_messages.is_empty() {
        object.insert("commit_messages".into(), json!(commit_messages));
    }
    if atom.review_thread_ids.len() > 0 {
        object.insert(
            "review_thread_count".into(),
            json!(atom.review_thread_ids.len()),
        );
    }
    object.insert("risk_score".into(), json!(atom.risk_score));
    let is_generated = atom.role == ChangeRole::Generated
        || matches!(atom.source, ChangeAtomSource::GeneratedPlaceholder);
    if is_generated {
        object.insert("is_generated".into(), json!(true));
    }
    if matches!(atom.source, ChangeAtomSource::BinaryPlaceholder) {
        object.insert("is_binary".into(), json!(true));
    }
    object.insert(
        "confidence".into(),
        json!(if requires_manual_review(atom) {
            "low"
        } else {
            "medium"
        }),
    );
    Value::Object(object)
}

fn commits_by_path(commits: &[CommitSummary]) -> BTreeMap<String, Vec<&CommitSummary>> {
    let mut by_path = BTreeMap::<String, Vec<&CommitSummary>>::new();
    for commit in commits {
        for path in &commit.paths {
            by_path.entry(path.clone()).or_default().push(commit);
        }
    }
    by_path
}

fn atom_title(atom: &ChangeAtom) -> String {
    atom.symbol_name
        .clone()
        .or_else(|| atom.hunk_headers.first().cloned())
        .unwrap_or_else(|| atom.path.clone())
}

fn atom_summary(atom: &ChangeAtom) -> String {
    let source = match &atom.source {
        ChangeAtomSource::File => "file-level change",
        ChangeAtomSource::Hunk { .. } => "diff hunk",
        ChangeAtomSource::SemanticSection { .. } => "semantic section",
        ChangeAtomSource::Commit { .. } => "commit atom",
        ChangeAtomSource::GeneratedPlaceholder => "generated or huge file placeholder",
        ChangeAtomSource::BinaryPlaceholder => "binary file placeholder",
    };
    format!(
        "{} in {} with {} changed line{}.",
        source,
        atom.path,
        atom.additions + atom.deletions,
        if atom.additions + atom.deletions == 1 {
            ""
        } else {
            "s"
        }
    )
}

fn missing_commit_context(commits_count: i64) -> CommitContext {
    CommitContext {
        commits: Vec::new(),
        suitability: CommitSuitability {
            score: 0.0,
            suitable_for_layers: false,
            reasons: vec![format!(
                "Commit metadata was unavailable; GitHub reported {commits_count} commit{}.",
                if commits_count == 1 { "" } else { "s" }
            )],
        },
    }
}

fn atom_refs_for_ids<'a>(
    atom_ids: &[ChangeAtomId],
    atoms_by_id: &'a BTreeMap<ChangeAtomId, &'a ChangeAtom>,
) -> Vec<&'a ChangeAtom> {
    atom_ids
        .iter()
        .filter_map(|atom_id| atoms_by_id.get(atom_id).copied())
        .collect()
}

fn metrics_for_atoms(atoms: &[&ChangeAtom]) -> LayerMetrics {
    let file_count = atoms
        .iter()
        .map(|atom| atom.path.as_str())
        .collect::<BTreeSet<_>>()
        .len();

    LayerMetrics {
        file_count,
        atom_count: atoms.len(),
        additions: atoms.iter().map(|atom| atom.additions).sum(),
        deletions: atoms.iter().map(|atom| atom.deletions).sum(),
        changed_lines: atoms
            .iter()
            .map(|atom| atom.additions + atom.deletions)
            .sum(),
        unresolved_thread_count: atoms.iter().map(|atom| atom.review_thread_ids.len()).sum(),
        risk_score: atoms.iter().map(|atom| atom.risk_score).sum(),
    }
}

fn dominant_role(atoms: &[&ChangeAtom]) -> ChangeRole {
    let mut counts = BTreeMap::<ChangeRole, usize>::new();
    for atom in atoms {
        *counts.entry(atom.role).or_default() += atom.additions + atom.deletions + 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(role, _)| role)
        .unwrap_or(ChangeRole::Unknown)
}

fn review_priority_label(priority: &AiReviewPriority) -> &'static str {
    match priority {
        AiReviewPriority::StartHere => "start_here",
        AiReviewPriority::Normal => "normal",
        AiReviewPriority::QuickPass => "quick_pass",
        AiReviewPriority::ManualReview => "manual_review",
    }
}

fn layer_summary_with_review_question(layer: &AiStackPlanLayer) -> String {
    if layer.review_question.trim().is_empty() {
        layer.summary.clone()
    } else {
        format!("{} {}", layer.review_question.trim(), layer.summary.trim())
    }
}

fn layer_rationale_with_review_question(layer: &AiStackPlanLayer) -> String {
    if layer.review_question.trim().is_empty() {
        layer.rationale.clone()
    } else {
        format!(
            "Review question: {}\n\n{}",
            layer.review_question.trim(),
            layer.rationale.trim()
        )
    }
}

fn clean_layer_text(value: &str, fallback: &str, limit: usize) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let truncated = trimmed
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    format!("{}...", truncated.trim_end())
}

fn virtual_stack_id(selected_pr: &PullRequestDetail) -> String {
    let mut hasher = Sha1::new();
    for part in [
        selected_pr.repository.as_str(),
        &selected_pr.number.to_string(),
        selected_pr.base_ref_oid.as_deref().unwrap_or_default(),
        selected_pr.head_ref_oid.as_deref().unwrap_or_default(),
        StackSource::VirtualAi.label(),
        STACK_GENERATOR_VERSION,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    format!("stack-{:x}", hasher.finalize())
}

fn virtual_layer_id(
    stack_id: &str,
    index: usize,
    role: ChangeRole,
    atom_ids: &[ChangeAtomId],
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(stack_id.as_bytes());
    hasher.update(index.to_string().as_bytes());
    hasher.update(role.label().as_bytes());
    for atom_id in atom_ids {
        hasher.update(atom_id.as_bytes());
    }
    format!("ai-virtual-layer-{}-{:x}", index, hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        github::{PullRequestDataCompleteness, PullRequestFile},
        stacks::{
            model::{LineRange, StackWarning},
            validation::{AiStackPlan, AiStackPlanStrategy},
        },
    };

    #[test]
    fn one_giant_commit_should_attempt_ai_planning() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation, 700),
            atom("atom_2", ChangeRole::CoreLogic, 900),
            atom("atom_3", ChangeRole::Integration, 600),
            atom("atom_4", ChangeRole::Tests, 300),
        ];
        let suitability = CommitSuitability {
            score: 0.1,
            suitable_for_layers: false,
            reasons: vec!["Only 1 commit for a large PR.".to_string()],
        };

        assert!(should_attempt_ai_planning(&atoms, 2_500, &suitability));
    }

    #[test]
    fn huge_two_commit_pr_should_attempt_ai_planning_before_commit_layers() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation, 900),
            atom("atom_2", ChangeRole::CoreLogic, 1_200),
            atom("atom_3", ChangeRole::Integration, 1_100),
            atom("atom_4", ChangeRole::Tests, 800),
        ];
        let suitability = CommitSuitability {
            score: 0.22,
            suitable_for_layers: false,
            reasons: vec![
                "Only 2 commits for a 4000-line PR.".to_string(),
                "Largest commit contains 82% of the changed lines.".to_string(),
            ],
        };

        assert!(should_attempt_ai_planning(&atoms, 4_000, &suitability));
    }

    #[test]
    fn builds_ai_stack_with_manual_review_final_layer() {
        let atoms = vec![
            atom("atom_1", ChangeRole::Foundation, 120),
            atom("atom_2", ChangeRole::CoreLogic, 180),
            manual_atom("atom_generated", 1_800),
        ];
        let plan = AiStackPlan {
            strategy: AiStackPlanStrategy::SemanticVirtualStack,
            confidence: Confidence::Medium,
            rationale: "Commits are too coarse.".to_string(),
            layers: vec![
                plan_layer("Foundation", vec!["atom_1"], vec![]),
                plan_layer("Core behavior", vec!["atom_2"], vec![0]),
            ],
            manual_review_atom_ids: vec!["atom_generated".to_string()],
            warnings: vec!["Generated file needs manual pass.".to_string()],
        };
        let validated = validate_ai_stack_plan(&plan, &atoms, 2_100).unwrap();
        let stack = build_stack_from_validated_plan(
            &detail(),
            atoms,
            validated,
            Some("test-model".to_string()),
            CodeTourProvider::Codex,
            CommitContext {
                commits: Vec::new(),
                suitability: CommitSuitability {
                    score: 0.0,
                    suitable_for_layers: false,
                    reasons: vec!["No commits.".to_string()],
                },
            },
        );

        assert_eq!(stack.source, StackSource::VirtualAi);
        assert_eq!(stack.layers.len(), 3);
        assert_eq!(
            stack.layers.last().unwrap().title,
            "Manual review / uncertain changes"
        );
        assert_eq!(
            stack
                .layers
                .iter()
                .flat_map(|layer| layer.atom_ids.iter())
                .collect::<BTreeSet<_>>()
                .len(),
            stack.atoms.len()
        );
    }

    #[test]
    fn parses_wrapped_camel_case_stack_plan() {
        let raw = r#"{
            "plan": {
                "strategy": "Dependency Chain",
                "confidence": "Medium",
                "rationale": "Group by review dependencies.",
                "reviewLayers": [
                    {
                        "title": "Introduce the contract",
                        "reviewQuestion": "Does the contract support the later behavior?",
                        "summary": "Foundation changes.",
                        "rationale": "Later layers use this contract.",
                        "atomIds": [{"id": "atom_1"}],
                        "dependsOnLayerIndexes": [],
                        "reviewPriority": "Start Here"
                    }
                ],
                "manualReviewAtomIds": [],
                "warnings": []
            }
        }"#;

        let plan = parse_ai_stack_plan_response(raw)
            .expect("plan should parse")
            .plan;

        assert_eq!(plan.strategy, AiStackPlanStrategy::DependencyChain);
        assert_eq!(plan.confidence, Confidence::Medium);
        assert_eq!(plan.layers.len(), 1);
        assert_eq!(plan.layers[0].review_priority, AiReviewPriority::StartHere);
        assert_eq!(plan.layers[0].atom_ids, vec!["atom_1".to_string()]);
    }

    #[test]
    fn defaults_nonessential_stack_plan_metadata() {
        let raw = r#"{
            "layers": [
                {
                    "title": "Build core behavior",
                    "summary": "Core behavior changes.",
                    "rationale": "This is the central review layer.",
                    "atom_ids": ["atom_1"]
                }
            ]
        }"#;

        let plan = parse_ai_stack_plan_response(raw)
            .expect("plan should parse")
            .plan;

        assert_eq!(plan.strategy, AiStackPlanStrategy::SemanticVirtualStack);
        assert_eq!(plan.confidence, Confidence::Medium);
        assert_eq!(plan.layers[0].confidence, Confidence::Medium);
        assert_eq!(plan.layers[0].review_priority, AiReviewPriority::Normal);
    }

    #[test]
    fn invalid_ai_output_records_ai_unavailable_warning() {
        let stack = ai_unavailable_stack(
            &detail(),
            "AI stack planning returned invalid output. Remiss did not generate a non-AI stack.",
            Some(json!({ "validationError": "omitted atom" })),
        )
        .expect("unavailable stack");

        assert_eq!(stack.source, StackSource::VirtualAi);
        assert_eq!(stack.layers[0].title, "AI stack unavailable");
        assert!(stack.warnings.iter().any(|warning| {
            warning.code == "ai-virtual-stack-unavailable"
                && warning.message.contains("invalid output")
        }));
    }

    fn plan_layer(title: &str, atom_ids: Vec<&str>, deps: Vec<usize>) -> AiStackPlanLayer {
        AiStackPlanLayer {
            title: title.to_string(),
            review_question: format!("Does this change correctly cover {title}?"),
            summary: format!("{title} summary"),
            rationale: format!("{title} rationale"),
            substantive_atom_ids: Vec::new(),
            attached_noise_atom_ids: Vec::new(),
            atom_ids: atom_ids.into_iter().map(str::to_string).collect(),
            depends_on_layer_indexes: deps,
            confidence: Confidence::Medium,
            review_priority: AiReviewPriority::Normal,
        }
    }

    fn atom(id: &str, role: ChangeRole, changed_lines: usize) -> ChangeAtom {
        ChangeAtom {
            id: id.to_string(),
            source: ChangeAtomSource::Hunk { hunk_index: 0 },
            path: format!("src/{id}.rs"),
            previous_path: None,
            role,
            semantic_kind: Some("function".to_string()),
            symbol_name: Some(id.to_string()),
            defined_symbols: vec![id.to_string()],
            referenced_symbols: Vec::new(),
            old_range: Some(LineRange { start: 1, end: 2 }),
            new_range: Some(LineRange { start: 1, end: 3 }),
            hunk_headers: Vec::new(),
            hunk_indices: vec![0],
            additions: changed_lines,
            deletions: 0,
            patch_hash: format!("hash-{id}"),
            risk_score: changed_lines as i64,
            review_thread_ids: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn manual_atom(id: &str, changed_lines: usize) -> ChangeAtom {
        let mut atom = atom(id, ChangeRole::Generated, changed_lines);
        atom.source = ChangeAtomSource::GeneratedPlaceholder;
        atom.warnings = vec![StackWarning::new("manual-review", "Generated file.")];
        atom
    }

    fn detail() -> PullRequestDetail {
        PullRequestDetail {
            id: "pr".to_string(),
            repository: "acme/repo".to_string(),
            number: 1,
            title: "PR".to_string(),
            body: String::new(),
            url: String::new(),
            author_login: "octo".to_string(),
            author_avatar_url: None,
            state: "OPEN".to_string(),
            is_draft: false,
            review_decision: None,
            base_ref_name: "main".to_string(),
            head_ref_name: "feature".to_string(),
            base_ref_oid: Some("base".to_string()),
            head_ref_oid: Some("head".to_string()),
            additions: 2_100,
            deletions: 0,
            changed_files: 3,
            comments_count: 0,
            commits_count: 1,
            created_at: String::new(),
            updated_at: "now".to_string(),
            labels: Vec::new(),
            reviewers: Vec::new(),
            reviewer_avatar_urls: Default::default(),
            comments: Vec::new(),
            latest_reviews: Vec::new(),
            review_threads: Vec::new(),
            files: vec![PullRequestFile {
                path: "src/atom_1.rs".to_string(),
                additions: 120,
                deletions: 0,
                change_type: "MODIFIED".to_string(),
            }],
            raw_diff: String::new(),
            parsed_diff: Vec::new(),
            data_completeness: PullRequestDataCompleteness::default(),
        }
    }
}
