import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { CopilotClient } from "@github/copilot-sdk";
import { Codex } from "@openai/codex-sdk";

const execFileAsync = promisify(execFile);

const GENERATION_TIMEOUT_MS = 600_000;
const STATUS_TIMEOUT_MS = 20_000;
const MAX_FILES = 80;
const MAX_REVIEWS = 5;
const MAX_THREADS = 12;
const MAX_COMMENTS_PER_THREAD = 3;
const MAX_SECTIONS = 10;
const MAX_REVIEW_POINTS = 4;
const MAX_CALLSITES_PER_SECTION = 3;
const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = resolve(SCRIPT_DIRECTORY, "..");

const TOUR_OUTPUT_SCHEMA = {
  type: "object",
  properties: {
    summary: { type: "string" },
    reviewFocus: { type: "string" },
    openQuestions: {
      type: "array",
      items: { type: "string" }
    },
    warnings: {
      type: "array",
      items: { type: "string" }
    },
    overview: {
      type: "object",
      properties: {
        title: { type: "string" },
        summary: { type: "string" },
        detail: { type: "string" },
        badge: { type: "string" }
      },
      required: ["title", "summary", "detail", "badge"],
      additionalProperties: false
    },
    steps: {
      type: "array",
      items: {
        type: "object",
        properties: {
          sourceStepId: { type: "string" },
          title: { type: "string" },
          summary: { type: "string" },
          detail: { type: "string" },
          badge: { type: "string" }
        },
        required: ["sourceStepId", "title", "summary", "detail", "badge"],
        additionalProperties: false
      }
    },
    sections: {
      type: "array",
      items: {
        type: "object",
        properties: {
          title: { type: "string" },
          summary: { type: "string" },
          detail: { type: "string" },
          badge: { type: "string" },
          stepIds: {
            type: "array",
            items: { type: "string" }
          },
          reviewPoints: {
            type: "array",
            items: { type: "string" }
          },
          callsites: {
            type: "array",
            items: {
              type: "object",
              properties: {
                title: { type: "string" },
                path: { type: "string" },
                line: { type: ["integer", "null"] },
                summary: { type: "string" },
                snippet: { type: ["string", "null"] }
              },
              required: ["title", "path", "line", "summary", "snippet"],
              additionalProperties: false
            }
          }
        },
        required: [
          "title",
          "summary",
          "detail",
          "badge",
          "stepIds",
          "reviewPoints",
          "callsites"
        ],
        additionalProperties: false
      }
    }
  },
  required: [
    "summary",
    "reviewFocus",
    "openQuestions",
    "warnings",
    "overview",
    "steps",
    "sections"
  ],
  additionalProperties: false
};

const request = JSON.parse(await readStdin());

try {
  if (request.action === "status") {
    process.stdout.write(JSON.stringify(await loadProviderStatuses()));
    process.exit(0);
  }

  if (request.action === "generate") {
    process.stdout.write(JSON.stringify(await generateCodeTour(request.context)));
    process.exit(0);
  }

  throw new Error(`Unsupported code tour bridge action: ${request.action}`);
} catch (error) {
  const message =
    error instanceof Error ? error.message : "Unknown code tour bridge failure.";
  process.stderr.write(message);
  process.exit(1);
}

async function loadProviderStatuses() {
  const [codex, copilot] = await Promise.all([
    loadCodexStatus(),
    loadCopilotStatus()
  ]);

  return {
    providers: [codex, copilot]
  };
}

async function loadCodexStatus() {
  const cliPath = resolveCliPath("codex");

  if (!cliPath) {
    return unavailableProviderStatus(
      "codex",
      "Codex",
      "Codex CLI is not installed in this app workspace."
    );
  }

  try {
    const { stdout, stderr } = await withTimeout(
      execFileAsync(cliPath, ["login", "status"], {
        cwd: process.cwd(),
        env: process.env
      }),
      STATUS_TIMEOUT_MS,
      "Timed out while checking Codex login status."
    );
    const message = firstLine(stdout) ?? firstLine(stderr) ?? "Codex status unavailable.";
    const authenticated = /logged in/i.test(message);

    return {
      provider: "codex",
      label: "Codex",
      available: true,
      authenticated,
      message,
      detail: authenticated
        ? "Uses the local Codex CLI session from your existing ChatGPT or API-backed setup."
        : "Sign in with `codex login` to use Codex for AI code tours.",
      defaultModel: null
    };
  } catch (error) {
    return {
      provider: "codex",
      label: "Codex",
      available: true,
      authenticated: false,
      message:
        error instanceof Error ? error.message : "Failed to check Codex status.",
      detail: "Codex is installed but not ready for tour generation yet.",
      defaultModel: null
    };
  }
}

async function loadCopilotStatus() {
  const cliPath = resolveCliPath("copilot");

  if (!cliPath) {
    return unavailableProviderStatus(
      "copilot",
      "Copilot",
      "GitHub Copilot CLI is not installed in this app workspace."
    );
  }

  const client = new CopilotClient({
    cliPath,
    cwd: process.cwd(),
    logLevel: "error"
  });

  try {
    await withTimeout(
      client.start(),
      STATUS_TIMEOUT_MS,
      "Timed out while starting GitHub Copilot CLI."
    );

    const auth = await withTimeout(
      client.getAuthStatus(),
      STATUS_TIMEOUT_MS,
      "Timed out while checking GitHub Copilot authentication."
    );
    const models = await loadCopilotModels(client);
    const defaultModel = selectCopilotModel(models);
    const message =
      auth.statusMessage ??
      (auth.isAuthenticated
        ? `Authenticated as ${auth.login ?? "the current user"}.`
        : "GitHub Copilot is not authenticated.");

    return {
      provider: "copilot",
      label: "Copilot",
      available: true,
      authenticated: auth.isAuthenticated,
      message,
      detail: auth.isAuthenticated
        ? "Uses the local GitHub Copilot CLI auth from your existing subscription."
        : "Sign in with `copilot login` or provide a Copilot-capable token to generate tours.",
      defaultModel
    };
  } catch (error) {
    return {
      provider: "copilot",
      label: "Copilot",
      available: true,
      authenticated: false,
      message:
        error instanceof Error ? error.message : "Failed to check Copilot status.",
      detail: "Copilot is installed but not ready for tour generation yet.",
      defaultModel: null
    };
  } finally {
    await client.stop().catch(() => []);
  }
}

async function generateCodeTour(input) {
  if (!input?.provider) {
    throw new Error("Code tour generation requires a provider.");
  }

  if (!input.workingDirectory || !existsSync(input.workingDirectory)) {
    throw new Error(
      "Code tours need a valid linked local repository so the LLM can inspect the code."
    );
  }

  if (!Array.isArray(input.candidateSteps) || input.candidateSteps.length === 0) {
    throw new Error("No candidate steps were provided for code tour generation.");
  }

  if (input.provider === "codex") {
    return generateWithCodex(input);
  }

  if (input.provider === "copilot") {
    return generateWithCopilot(input);
  }

  throw new Error(`Unsupported code tour provider: ${input.provider}`);
}

async function generateWithCodex(input) {
  const cliPath = resolveCliPath("codex");

  if (!cliPath) {
    throw new Error("Codex CLI is not available in this workspace.");
  }

  const codex = new Codex({
    codexPathOverride: cliPath
  });
  const thread = codex.startThread({
    workingDirectory: input.workingDirectory,
    sandboxMode: "read-only",
    modelReasoningEffort: "medium",
    approvalPolicy: "never",
    networkAccessEnabled: false,
    webSearchMode: "disabled"
  });
  const controller = new AbortController();
  let timedOut = false;
  const timeout = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, GENERATION_TIMEOUT_MS);

  try {
    const result = await thread.run(buildTourPrompt(input), {
      outputSchema: TOUR_OUTPUT_SCHEMA,
      signal: controller.signal
    });

    return {
      tour: mergeTour(parseStructuredResponse(result.finalResponse), input, null)
    };
  } catch (error) {
    if (timedOut || controller.signal.aborted || isAbortError(error)) {
      throw new Error(
        "Codex timed out while generating the code tour after 10 minutes."
      );
    }

    throw error;
  } finally {
    clearTimeout(timeout);
  }
}

async function generateWithCopilot(input) {
  const cliPath = resolveCliPath("copilot");

  if (!cliPath) {
    throw new Error("GitHub Copilot CLI is not available in this workspace.");
  }

  const client = new CopilotClient({
    cliPath,
    cwd: input.workingDirectory,
    logLevel: "error"
  });

  try {
    await withTimeout(
      client.start(),
      STATUS_TIMEOUT_MS,
      "Timed out while starting GitHub Copilot."
    );

    const auth = await withTimeout(
      client.getAuthStatus(),
      STATUS_TIMEOUT_MS,
      "Timed out while checking GitHub Copilot authentication."
    );

    if (!auth.isAuthenticated) {
      throw new Error(
        auth.statusMessage ??
          "GitHub Copilot is not authenticated. Run `copilot login` first."
      );
    }

    const models = await loadCopilotModels(client);
    const model = selectCopilotModel(models);
    const session = await withTimeout(
      client.createSession({
        model: model ?? undefined,
        workingDirectory: input.workingDirectory,
        streaming: false,
        systemMessage: {
          content: [
            "You are helping generate a code-review tour inside a desktop pull-request tool.",
            "Stay read-only. Never edit files or claim you made changes.",
            "Return strict JSON with no markdown fences."
          ].join(" ")
        },
        onPermissionRequest(request) {
          if (request.kind === "read") {
            return { kind: "approved" };
          }

          return { kind: "denied-by-rules" };
        }
      }),
      STATUS_TIMEOUT_MS,
      "Timed out while creating the GitHub Copilot session."
    );

    try {
      const response = await withTimeout(
        session.sendAndWait(
          {
            prompt: buildTourPrompt(input)
          },
          GENERATION_TIMEOUT_MS
        ),
        GENERATION_TIMEOUT_MS + 5_000,
        "GitHub Copilot timed out while generating the code tour."
      );
      const content = response?.data?.content?.trim();

      if (!content) {
        throw new Error("GitHub Copilot returned an empty code tour response.");
      }

      return {
        tour: mergeTour(parseStructuredResponse(content), input, model)
      };
    } finally {
      await session.disconnect().catch(() => {});
    }
  } finally {
    await client.stop().catch(() => []);
  }
}

function buildTourPrompt(input) {
  const promptContext = buildPromptContext(input);

  return [
    "You are generating a guided code tour for a GitHub pull request.",
    "Act like a senior pair programmer walking a reviewer through the change.",
    "Assume the reviewer already knows the codebase well. Be direct, useful, and never condescending.",
    "Stay grounded in the provided pull-request data and the linked local repository.",
    "Do not edit files, propose patches, or imply that you changed the code.",
    "Return JSON only with no markdown fences or extra commentary.",
    "Always use the provided candidate step ids. Never invent ids.",
    "Explain the whole pull request first, then organize the changed files into related sections.",
    "Use the section stepIds to cover the whole changeset. Reuse each candidate file step at most once across sections.",
    "Each step summary should be one sentence. Each detail should be 1 to 3 sentences focused on what changed, why it matters, and what to verify in review.",
    "Each section should explain why those files belong together and how the change moves across them.",
    "For new or materially changed APIs, helpers, components, types, or commands, include concrete verified callsites when they help teach the change.",
    "Only include callsites you can support from the linked checkout. Keep callsite snippets compact.",
    "Surface unresolved review concerns in openQuestions when appropriate.",
    "",
    "JSON schema:",
    JSON.stringify(TOUR_OUTPUT_SCHEMA, null, 2),
    "",
    "Pull-request context:",
    JSON.stringify(promptContext, null, 2)
  ].join("\n");
}

function buildPromptContext(input) {
  const [overviewStep, ...fileSteps] = input.candidateSteps;
  const prioritizedThreads = [...input.reviewThreads].sort((left, right) => {
    if (left.isResolved === right.isResolved) {
      return 0;
    }

    return left.isResolved ? 1 : -1;
  });

  return {
    repository: input.repository,
    workingDirectory: input.workingDirectory,
    pullRequest: {
      number: input.number,
      title: input.title,
      url: input.url,
      authorLogin: input.authorLogin,
      reviewDecision: input.reviewDecision,
      baseRefName: input.baseRefName,
      headRefName: input.headRefName,
      updatedAt: input.updatedAt,
      stats: {
        commits: input.commitsCount,
        changedFiles: input.changedFiles,
        additions: input.additions,
        deletions: input.deletions
      },
      body: limitText(input.body, 2_500)
    },
    files: input.files.slice(0, MAX_FILES).map((file) => ({
      path: file.path,
      changeType: file.changeType,
      additions: file.additions,
      deletions: file.deletions
    })),
    latestReviews: input.latestReviews.slice(0, MAX_REVIEWS).map((review) => ({
      authorLogin: review.authorLogin,
      state: review.state,
      submittedAt: review.submittedAt,
      body: limitText(review.body, 900)
    })),
    reviewThreads: prioritizedThreads.slice(0, MAX_THREADS).map((thread) => ({
      path: thread.path,
      line: thread.line,
      diffSide: thread.diffSide,
      subjectType: thread.subjectType,
      isResolved: thread.isResolved,
      comments: thread.comments.slice(0, MAX_COMMENTS_PER_THREAD).map((comment) => ({
        authorLogin: comment.authorLogin,
        body: limitText(comment.body, 500)
      }))
    })),
    overviewStep: summarizeStep(overviewStep),
    candidateGroups: ensureArray(input.candidateGroups).map((group) => ({
      id: group.id,
      title: group.title,
      summary: group.summary,
      stepIds: ensureArray(group.stepIds),
      filePaths: ensureArray(group.filePaths)
    })),
    candidateSteps: fileSteps.map(summarizeStep)
  };
}

function summarizeStep(step) {
  if (!step) {
    return null;
  }

  return {
    id: step.id,
    kind: step.kind,
    title: step.title,
    summary: step.summary,
    detail: step.detail,
    badge: step.badge,
    filePath: step.filePath,
    anchor: step.anchor,
    additions: step.additions,
    deletions: step.deletions,
    unresolvedThreadCount: step.unresolvedThreadCount,
    snippet: step.snippet ? limitText(step.snippet, 500) : null
  };
}

function mergeTour(response, input, model) {
  const candidateStepsById = new Map(
    input.candidateSteps.map((step) => [step.id, step])
  );
  const overviewCandidate =
    candidateStepsById.get("overview") ?? input.candidateSteps[0] ?? null;
  const mergedStepsById = new Map();

  let overviewStep = null;
  if (overviewCandidate) {
    overviewStep = mergeStep(overviewCandidate, response.overview);
    mergedStepsById.set(overviewStep.id, overviewStep);
  }

  for (const candidate of input.candidateSteps) {
    if (candidate.kind !== "file") {
      continue;
    }

    const override = ensureArray(response.steps).find(
      (item) => item?.sourceStepId === candidate.id
    );
    mergedStepsById.set(candidate.id, mergeStep(candidate, override));
  }

  const usedStepIds = new Set();
  const mergedSections = [];

  for (const item of ensureArray(response.sections).slice(0, MAX_SECTIONS)) {
    const stepIds = uniqueStepIds(item?.stepIds).filter((stepId) => {
      const candidate = candidateStepsById.get(stepId);

      return candidate?.kind === "file" && !usedStepIds.has(stepId);
    });

    if (stepIds.length === 0) {
      continue;
    }

    const sectionSteps = stepIds
      .map((stepId) => mergedStepsById.get(stepId))
      .filter(Boolean);

    mergedSections.push({
      id: `section:${mergedSections.length + 1}`,
      title: fallbackText(item?.title, fallbackSectionTitle(sectionSteps)),
      summary: fallbackText(item?.summary, fallbackSectionSummary(sectionSteps)),
      detail: fallbackText(item?.detail, fallbackSectionDetail(sectionSteps)),
      badge: fallbackText(item?.badge, fallbackSectionBadge(sectionSteps)),
      stepIds,
      reviewPoints: sanitizeStringArray(item?.reviewPoints, MAX_REVIEW_POINTS),
      callsites: sanitizeCallsites(item?.callsites, MAX_CALLSITES_PER_SECTION)
    });

    for (const stepId of stepIds) {
      usedStepIds.add(stepId);
    }
  }

  for (const section of buildFallbackSections(input, mergedStepsById, usedStepIds)) {
    mergedSections.push({
      ...section,
      id: `section:${mergedSections.length + 1}`
    });
  }

  const mergedSteps = [];

  if (overviewStep) {
    mergedSteps.push(overviewStep);
  }

  for (const section of mergedSections) {
    for (const stepId of section.stepIds) {
      const step = mergedStepsById.get(stepId);

      if (step) {
        mergedSteps.push(step);
      }
    }
  }

  return {
    provider: input.provider,
    model,
    generatedAt: new Date().toISOString(),
    summary: fallbackText(
      response.summary,
      "AI-generated tour focused on the most reviewable parts of this pull request."
    ),
    reviewFocus: fallbackText(
      response.reviewFocus,
      "Review the walkthrough section by section and use the diff anchors to drop into the concrete implementation details."
    ),
    openQuestions: sanitizeStringArray(response.openQuestions, 6),
    warnings: sanitizeStringArray(response.warnings, 4),
    sections: mergedSections,
    steps: mergedSteps
  };
}

async function loadCopilotModels(client) {
  try {
    return await withTimeout(
      client.listModels(),
      STATUS_TIMEOUT_MS,
      "Timed out while listing GitHub Copilot models."
    );
  } catch {
    return [];
  }
}

function selectCopilotModel(models) {
  if (!Array.isArray(models) || models.length === 0) {
    return null;
  }

  const preferredMatch =
    models.find((model) => model.id === "gpt-5") ??
    models.find((model) => model.id?.startsWith("gpt-5")) ??
    models.find((model) => model.id?.startsWith("o")) ??
    models[0];

  return preferredMatch?.id ?? null;
}

function unavailableProviderStatus(provider, label, message) {
  return {
    provider,
    label,
    available: false,
    authenticated: false,
    message,
    detail: "Install the required CLI and dependencies before using this provider.",
    defaultModel: null
  };
}

function resolveCliPath(name) {
  const localCandidate = resolve(PROJECT_ROOT, "node_modules", ".bin", name);
  return existsSync(localCandidate) ? localCandidate : name;
}

function fallbackText(value, fallback) {
  const normalized = typeof value === "string" ? value.trim() : "";
  return normalized.length > 0 ? normalized : fallback;
}

function sanitizeStringArray(value, limit) {
  if (!Array.isArray(value)) {
    return [];
  }

  const seen = new Set();
  const items = [];

  for (const entry of value) {
    if (typeof entry !== "string") {
      continue;
    }

    const trimmed = entry.trim();
    if (trimmed.length === 0 || seen.has(trimmed)) {
      continue;
    }

    seen.add(trimmed);
    items.push(trimmed);

    if (items.length >= limit) {
      break;
    }
  }

  return items;
}

function sanitizeCallsites(value, limit) {
  if (!Array.isArray(value)) {
    return [];
  }

  const seen = new Set();
  const items = [];

  for (const entry of value) {
    if (!entry || typeof entry !== "object") {
      continue;
    }

    const path = sanitizeString(entry.path);
    const summary = sanitizeString(entry.summary);

    if (!path || !summary) {
      continue;
    }

    const line =
      typeof entry.line === "number" && Number.isFinite(entry.line)
        ? Math.max(1, Math.floor(entry.line))
        : null;
    const title =
      sanitizeString(entry.title) ?? `Callsite in ${path}${line ? `:${line}` : ""}`;
    const snippet = sanitizeOptionalString(entry.snippet, 1_200);
    const key = `${path}:${line ?? "no-line"}:${title}`;

    if (seen.has(key)) {
      continue;
    }

    seen.add(key);
    items.push({
      title,
      path,
      line,
      summary,
      snippet
    });

    if (items.length >= limit) {
      break;
    }
  }

  return items;
}

function sanitizeString(value) {
  if (typeof value !== "string") {
    return null;
  }

  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

function sanitizeOptionalString(value, maxLength) {
  const normalized = sanitizeString(value);
  return normalized ? limitText(normalized, maxLength) : null;
}

function uniqueStepIds(value) {
  if (!Array.isArray(value)) {
    return [];
  }

  const seen = new Set();
  const stepIds = [];

  for (const entry of value) {
    if (typeof entry !== "string") {
      continue;
    }

    const trimmed = entry.trim();
    if (!trimmed || seen.has(trimmed)) {
      continue;
    }

    seen.add(trimmed);
    stepIds.push(trimmed);
  }

  return stepIds;
}

function mergeStep(candidate, override) {
  return {
    ...candidate,
    title: fallbackText(override?.title, candidate.title),
    summary: fallbackText(override?.summary, candidate.summary),
    detail: fallbackText(override?.detail, candidate.detail),
    badge: fallbackText(override?.badge, candidate.badge)
  };
}

function buildFallbackSections(input, mergedStepsById, usedStepIds) {
  const sections = [];

  for (const group of ensureArray(input.candidateGroups)) {
    const stepIds = uniqueStepIds(group?.stepIds).filter((stepId) => {
      const step = mergedStepsById.get(stepId);
      return step?.kind === "file" && !usedStepIds.has(stepId);
    });

    if (stepIds.length === 0) {
      continue;
    }

    const sectionSteps = stepIds
      .map((stepId) => mergedStepsById.get(stepId))
      .filter(Boolean);

    sections.push({
      id: "",
      title: fallbackText(group?.title, fallbackSectionTitle(sectionSteps)),
      summary: fallbackText(group?.summary, fallbackSectionSummary(sectionSteps)),
      detail: fallbackSectionDetail(sectionSteps),
      badge: fallbackSectionBadge(sectionSteps),
      stepIds,
      reviewPoints: fallbackSectionReviewPoints(sectionSteps),
      callsites: []
    });

    for (const stepId of stepIds) {
      usedStepIds.add(stepId);
    }
  }

  const remainingStepIds = [...mergedStepsById.values()]
    .filter((step) => step.kind === "file" && !usedStepIds.has(step.id))
    .map((step) => step.id);

  if (remainingStepIds.length > 0) {
    const sectionSteps = remainingStepIds
      .map((stepId) => mergedStepsById.get(stepId))
      .filter(Boolean);

    sections.push({
      id: "",
      title: "Remaining changed files",
      summary: fallbackSectionSummary(sectionSteps),
      detail: fallbackSectionDetail(sectionSteps),
      badge: fallbackSectionBadge(sectionSteps),
      stepIds: remainingStepIds,
      reviewPoints: fallbackSectionReviewPoints(sectionSteps),
      callsites: []
    });
  }

  return sections;
}

function fallbackSectionTitle(steps) {
  if (!Array.isArray(steps) || steps.length === 0) {
    return "Related changes";
  }

  if (steps.length === 1) {
    return steps[0].title;
  }

  return `Related changes across ${steps.length} files`;
}

function fallbackSectionSummary(steps) {
  if (!Array.isArray(steps) || steps.length === 0) {
    return "Grouped changes from the pull request.";
  }

  const additions = steps.reduce((total, step) => total + (step.additions ?? 0), 0);
  const deletions = steps.reduce((total, step) => total + (step.deletions ?? 0), 0);
  const unresolvedThreadCount = steps.reduce(
    (total, step) => total + (step.unresolvedThreadCount ?? 0),
    0
  );
  const delta = `+${additions} / -${deletions}`;

  if (unresolvedThreadCount > 0) {
    return `${steps.length} related files with ${delta} and ${unresolvedThreadCount} unresolved review threads.`;
  }

  return `${steps.length} related files with ${delta}.`;
}

function fallbackSectionDetail(steps) {
  if (steps.some((step) => (step.unresolvedThreadCount ?? 0) > 0)) {
    return "Review these files together and carry the open review discussion across the whole slice of the change.";
  }

  return "Review these files together to trace how the behavior moves through this part of the repository before dropping into the raw diff.";
}

function fallbackSectionBadge(steps) {
  if (steps.some((step) => (step.unresolvedThreadCount ?? 0) > 0)) {
    return "discussion";
  }

  if (steps.some((step) => step.badge === "added")) {
    return "new surface";
  }

  if (steps.length > 1) {
    return "grouped";
  }

  return steps[0]?.badge ?? "focus";
}

function fallbackSectionReviewPoints(steps) {
  const points = [];

  if (steps.length > 1) {
    points.push(
      `Trace the flow across ${steps.length} files here instead of reviewing each patch in isolation.`
    );
  }

  if (steps.some((step) => step.badge === "added")) {
    points.push("Check how the new entry points or data shapes connect back to existing callers.");
  }

  if (steps.some((step) => (step.unresolvedThreadCount ?? 0) > 0)) {
    points.push("Keep the unresolved review discussion in view while checking the rest of this section.");
  }

  if (points.length === 0) {
    points.push("Use the file cards below to inspect the concrete diff anchors for this section.");
  }

  return points.slice(0, MAX_REVIEW_POINTS);
}

function ensureArray(value) {
  return Array.isArray(value) ? value : [];
}

function limitText(value, maxLength) {
  if (!value) {
    return "";
  }

  const normalized = String(value).trim();

  if (normalized.length <= maxLength) {
    return normalized;
  }

  return `${normalized.slice(0, maxLength - 1).trimEnd()}…`;
}

function parseStructuredResponse(raw) {
  if (!raw || typeof raw !== "string") {
    throw new Error("The LLM returned an empty response.");
  }

  const extractedJson = extractFirstJsonObject(raw);

  if (!extractedJson) {
    throw new Error("The LLM response did not contain a valid JSON object.");
  }

  try {
    return JSON.parse(extractedJson);
  } catch (error) {
    throw new Error(
      `Failed to parse the LLM response as JSON: ${
        error instanceof Error ? error.message : "unknown error"
      }`
    );
  }
}

function extractFirstJsonObject(value) {
  const start = value.indexOf("{");

  if (start < 0) {
    return null;
  }

  let depth = 0;
  let inString = false;
  let escaped = false;

  for (let index = start; index < value.length; index += 1) {
    const character = value[index];

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === '"') {
        inString = false;
      }

      continue;
    }

    if (character === '"') {
      inString = true;
      continue;
    }

    if (character === "{") {
      depth += 1;
      continue;
    }

    if (character === "}") {
      depth -= 1;

      if (depth === 0) {
        return value.slice(start, index + 1);
      }
    }
  }

  return null;
}

function firstLine(value) {
  return value
    ?.split("\n")
    .map((line) => line.trim())
    .find(Boolean);
}

function isAbortError(error) {
  return (
    error instanceof Error &&
    (error.name === "AbortError" ||
      error.message === "The operation was aborted." ||
      error.message.includes("aborted"))
  );
}

async function readStdin() {
  const chunks = [];

  for await (const chunk of process.stdin) {
    chunks.push(chunk);
  }

  return Buffer.concat(chunks).toString("utf8");
}

async function withTimeout(promise, timeoutMs, message) {
  let timeoutId = null;

  const timeoutPromise = new Promise((_, reject) => {
    timeoutId = setTimeout(() => reject(new Error(message)), timeoutMs);
  });

  try {
    return await Promise.race([promise, timeoutPromise]);
  } finally {
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
    }
  }
}
