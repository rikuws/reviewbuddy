import { accessSync, appendFileSync, constants, existsSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, relative, resolve } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { CopilotClient } from "@github/copilot-sdk";
import { Codex } from "@openai/codex-sdk";
import { jsonrepair } from "jsonrepair";

const execFileAsync = promisify(execFile);

const GENERATION_TIMEOUT_MS = 240_000;
const GENERATION_INACTIVITY_TIMEOUT_MS = 60_000;
const STATUS_TIMEOUT_MS = 20_000;
const PROGRESS_PREFIX = "__GH_UI_PROGRESS__";
const CODE_TOUR_LOG_DIR_ENV = "GH_UI_CODE_TOUR_LOG_DIR";
const MAX_FILES = 80;
const MAX_REVIEWS = 5;
const MAX_THREADS = 12;
const MAX_COMMENTS_PER_THREAD = 3;
const MAX_SECTIONS = 10;
const MAX_REVIEW_POINTS = 4;
const MAX_CALLSITES_PER_SECTION = 3;
const COPILOT_TOUR_AVAILABLE_TOOLS = ["report_intent", "view", "rg", "glob"];
const COPILOT_TOUR_READ_TOOLS = new Set(["view", "rg", "glob"]);
const MAX_COPILOT_VIEW_RANGE_LINES = 260;
const MAX_COPILOT_RG_HEAD_LIMIT = 40;
const MAX_COPILOT_SUPPORTING_PATH_USES = 2;
const MIN_COPILOT_READ_TOOL_BUDGET = 8;
const MAX_COPILOT_READ_TOOL_BUDGET = 24;
const COPILOT_READ_TOOL_BUDGET_BUFFER = 8;
const MIN_COPILOT_SUPPORTING_TOOL_BUDGET = 4;
const MAX_COPILOT_SUPPORTING_TOOL_BUDGET = 10;
const PROJECT_ROOT = resolveProjectRoot();
let currentRunLogger = null;

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

main().catch((error) => {
  const message =
    error instanceof Error ? error.message : "Unknown code tour bridge failure.";
  writeRunLog("run.failed", {
    message,
    stack: error instanceof Error ? error.stack : null
  });
  process.stderr.write(formatErrorMessageWithLogPath(message));
  process.exit(1);
});

async function main() {
  const request = JSON.parse(await readStdin());
  currentRunLogger = createRunLoggerForRequest(request);
  writeRunLog("run.start", summarizeRequestForLog(request));

  if (request.action === "status") {
    process.stdout.write(JSON.stringify(await loadProviderStatuses()));
    process.exit(0);
  }

  if (request.action === "generate") {
    const response = await generateCodeTour(request.context);
    writeRunLog("run.completed", {
      provider: sanitizeString(request?.context?.provider),
      repository: sanitizeString(request?.context?.repository),
      number:
        typeof request?.context?.number === "number" &&
        Number.isFinite(request.context.number)
          ? Math.floor(request.context.number)
          : null
    });
    process.stdout.write(JSON.stringify(response));
    process.exit(0);
  }

  throw new Error(`Unsupported code tour bridge action: ${request.action}`);
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
      "Codex CLI is not installed in this workspace or on PATH."
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
        ? "Uses the detected Codex CLI session."
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
      "GitHub Copilot CLI is not installed in this workspace or on PATH."
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
        ? "Uses the detected GitHub Copilot CLI session."
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
  emitDebugLogLocation();

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

  writeRunLog("generate.request", {
    provider: input.provider,
    repository: input.repository,
    number: input.number,
    workingDirectory: input.workingDirectory,
    candidateStepCount: ensureArray(input.candidateSteps).length,
    candidateGroupCount: ensureArray(input.candidateGroups).length,
    fileCount: ensureArray(input.files).length,
    reviewThreadCount: ensureArray(input.reviewThreads).length
  });

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

  emitProgress({
    stage: "startup",
    summary: "Starting Codex",
    detail: "Launching the local Codex CLI in the linked checkout.",
    log: "Starting Codex CLI"
  });
  writeRunLog("codex.start", {
    cliPath,
    workingDirectory: input.workingDirectory
  });
  const codex = new Codex({
    codexPathOverride: cliPath
  });
  const thread = codex.startThread({
    workingDirectory: input.workingDirectory,
    sandboxMode: "read-only",
    modelReasoningEffort: "low",
    approvalPolicy: "never",
    networkAccessEnabled: false,
    webSearchMode: "disabled"
  });
  const controller = new AbortController();
  const watchdog = createGenerationWatchdog((abortReason) => {
    emitProgress({
      stage: "timeout",
      summary: generationAbortMessage("Codex", abortReason),
      detail:
        "Aborting the Codex run so the app can surface the failure without waiting for the old 10 minute timeout.",
      log: generationAbortMessage("Codex", abortReason)
    });
    controller.abort();
  });

  try {
    const prompt = buildTourPrompt(input);
    writeRunLog("prompt.prepared", {
      provider: "codex",
      promptLength: prompt.length,
      promptPreview: limitText(prompt, 4_000),
      candidateStepCount: ensureArray(input.candidateSteps).length,
      candidateGroupCount: ensureArray(input.candidateGroups).length
    });
    trackProgress(watchdog, {
      stage: "planning",
      summary: "Codex is reading the tour request",
      detail:
        "Inspecting the pull request context before it opens files from the linked checkout.",
      log: "Reading the code tour request"
    });
    const { events } = await thread.runStreamed(prompt, {
      outputSchema: TOUR_OUTPUT_SCHEMA,
      signal: controller.signal
    });
    let finalResponse = null;

    for await (const event of events) {
      const progress = progressForCodexEvent(event);
      if (progress) {
        trackProgress(watchdog, progress);
      } else {
        watchdog.touchBackground();
      }

      if (event.type === "item.completed" && event.item.type === "agent_message") {
        finalResponse = event.item.text;
      } else if (event.type === "turn.failed") {
        throw new Error(event.error?.message ?? "Codex failed while generating the code tour.");
      }
    }

    if (!sanitizeString(finalResponse)) {
      throw new Error("Codex returned an empty code tour response.");
    }

    writeRunLog("codex.response.received", {
      contentLength: finalResponse.length,
      contentPreview: limitText(finalResponse, 4_000)
    });

    trackProgress(watchdog, {
      stage: "finalizing",
      summary: "Codex finished the draft",
      detail: "Parsing the structured response and merging it into the final code tour.",
      log: "Finalizing Codex output"
    });

    return {
      tour: mergeTour(parseStructuredResponse(finalResponse), input, null)
    };
  } catch (error) {
    if (watchdog.abortReason() || controller.signal.aborted || isAbortError(error)) {
      throw new Error(
        generationAbortMessage(
          "Codex",
          watchdog.abortReason() ?? {
            kind: "overall",
            timeoutMs: GENERATION_TIMEOUT_MS
          }
        )
      );
    }

    throw error;
  } finally {
    watchdog.clear();
  }
}

async function generateWithCopilot(input) {
  const cliPath = resolveCliPath("copilot");

  if (!cliPath) {
    throw new Error("GitHub Copilot CLI is not available in this workspace.");
  }

  emitProgress({
    stage: "startup",
    summary: "Starting GitHub Copilot",
    detail: "Launching the local Copilot CLI in the linked checkout.",
    log: "Starting Copilot CLI"
  });
  writeRunLog("copilot.start", {
    cliPath,
    workingDirectory: input.workingDirectory
  });
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

    emitProgress({
      stage: "auth",
      summary: "Checking GitHub Copilot authentication",
      detail: "Verifying the current Copilot session before generating the code tour.",
      log: "Checking Copilot authentication"
    });
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

    emitProgress({
      stage: "model_lookup",
      summary: "Loading GitHub Copilot models",
      detail: "Choosing the preferred model and reasoning level for the code tour run.",
      log: "Loading Copilot models"
    });
    const models = await loadCopilotModels(client);
    const model = selectCopilotModel(models);
    const reasoningEffort = selectCopilotReasoningEffort(models, model);
    const orchestration = createCopilotTurnOrchestration(input);
    writeRunLog("copilot.models", {
      availableModels: ensureArray(models)
        .map((candidate) => sanitizeString(candidate?.id))
        .filter(Boolean),
      selectedModel: model,
      selectedReasoningEffort: reasoningEffort
    });
    emitProgress({
      stage: "session",
      summary: "Creating GitHub Copilot session",
      detail: model
        ? reasoningEffort
          ? `Using ${model} with ${reasoningEffort} reasoning effort for the code tour run.`
          : `Using ${model} for the code tour run.`
        : "Using the default Copilot model for the code tour run.",
      log: model
        ? reasoningEffort
          ? `Selected model ${model} (${reasoningEffort} reasoning)`
          : `Selected model ${model}`
        : "Using the default Copilot model"
    });
    const session = await withTimeout(
      client.createSession({
        model: model ?? undefined,
        reasoningEffort: reasoningEffort ?? undefined,
        availableTools: COPILOT_TOUR_AVAILABLE_TOOLS,
        workingDirectory: input.workingDirectory,
        streaming: true,
        hooks: orchestration.hooks,
        systemMessage: {
          content: [
            "You are helping generate a code-review tour inside a desktop pull-request tool.",
            "Stay read-only. Never edit files or claim you made changes.",
            "Work directly in the current session. Do not spawn sub-agents, background agents, or web searches.",
            "Use only these read-only tools when needed: report_intent, view, rg, and glob. Never use shell or git commands in this session.",
            "Finish the task in a single turn. Do not wait for follow-up instructions.",
            "Prefer the provided pull-request context and candidate snippets first.",
            "Only inspect files from the checkout when they are needed to verify a concrete claim.",
            "If a candidate file is missing from the checkout, continue with the provided pull-request context and snippets instead of trying recovery commands.",
            "After you have enough evidence for a best-effort tour, stop using tools and return the JSON immediately.",
            "If a search or file read is inconclusive, do not keep broadening the search; continue with the verified context you already have.",
            "Return strict JSON with no markdown fences."
          ].join(" ")
        },
        onPermissionRequest(request) {
          const kind = sanitizeString(request?.kind);
          const toolName = sanitizeString(request?.toolName);
          if (
            kind === "read" ||
            (kind === "custom-tool" &&
              toolName &&
              COPILOT_TOUR_AVAILABLE_TOOLS.includes(toolName))
          ) {
            return { kind: "approved" };
          }

          writeRunLog("copilot.permission.denied", {
            kind,
            toolName,
            fullCommandText: sanitizeOptionalString(request?.fullCommandText, 240)
          });

          return { kind: "denied-by-rules" };
        }
      }),
      STATUS_TIMEOUT_MS,
      "Timed out while creating the GitHub Copilot session."
    );

    try {
      const prompt = buildTourPrompt(input);
      writeRunLog("prompt.prepared", {
        provider: "copilot",
        promptLength: prompt.length,
        promptPreview: limitText(prompt, 4_000),
        candidateStepCount: ensureArray(input.candidateSteps).length,
        candidateGroupCount: ensureArray(input.candidateGroups).length
      });
      let response;
      try {
        response = await waitForCopilotTurn(
          session,
          prompt,
          input.workingDirectory,
          orchestration
        );
      } catch (error) {
        return {
          tour: buildCopilotFallbackTour(input, model, error, orchestration)
        };
      }
      const content = response?.data?.content?.trim();
      writeRunLog("copilot.response.received", {
        hasResponse: Boolean(response),
        contentLength: content?.length ?? 0,
        contentPreview: content ? limitText(content, 4_000) : null
      });

      if (!content) {
        return {
          tour: buildCopilotFallbackTour(
            input,
            model,
            new Error("GitHub Copilot returned an empty code tour response."),
            orchestration
          )
        };
      }

      emitProgress({
        stage: "finalizing",
        summary: "GitHub Copilot finished the draft",
        detail: "Parsing the structured response and merging it into the final code tour.",
        log: "Finalizing Copilot output"
      });
      try {
        return {
          tour: mergeTour(parseStructuredResponse(content), input, model)
        };
      } catch (error) {
        return {
          tour: buildCopilotFallbackTour(input, model, error, orchestration)
        };
      }
    } finally {
      orchestration.clearAbortHandler();
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
    "Finish the whole task in this turn. Do not wait for more instructions.",
    "Be fast and selective. Do not exhaustively explore the repository.",
    "Use only the available read-only tools: report_intent, view, rg, and glob.",
    "Start from the provided candidate groups and candidate steps before opening more files.",
    "Inspect only the changed files plus direct supporting callsites.",
    "Do not spawn sub-agents or background agents, and never try shell or git recovery commands.",
    "Inspect at most one targeted supporting callsite per section beyond the changed files. Once the story is clear, stop using tools and return the final JSON immediately.",
    "Do not reopen the same supporting file more than twice. If you already inspected a supporting file, treat that as enough evidence unless the first read was clearly insufficient.",
    "If a candidate file is missing from the checkout, treat it as deleted, renamed, or out-of-sync and continue with the provided pull-request context, snippets, and remaining files.",
    "If a supporting callsite cannot be verified quickly, omit it instead of continuing to search.",
    "If a search returns no direct hit, do not keep widening it. Continue with the verified pull-request context you already have.",
    "A complete best-effort tour is better than an exhaustive investigation.",
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

function createCopilotTurnOrchestration(input) {
  const candidatePaths = new Set(
    ensureArray(input.candidateSteps)
      .map((step) => normalizeRepositoryPath(step?.filePath, input.workingDirectory))
      .filter(Boolean)
  );
  const inspectedCandidatePaths = new Set();
  const supportingPathCounts = new Map();
  let totalReadToolCalls = 0;
  let totalSupportingToolCalls = 0;
  let stopRequestedReason = null;
  let hardStopReason = null;
  let abortHandler = null;
  let announcedCandidateCoverage = false;
  const maxReadToolCalls = computeCopilotReadToolBudget(candidatePaths.size);
  const maxSupportingToolCalls = computeCopilotSupportingToolBudget(candidatePaths.size);

  function snapshot() {
    return {
      candidatePathCount: candidatePaths.size,
      inspectedCandidatePathCount: inspectedCandidatePaths.size,
      totalReadToolCalls,
      totalSupportingToolCalls,
      maxReadToolCalls,
      maxSupportingToolCalls,
      stopRequestedReason,
      hardStopReason,
      supportingPathCounts: Object.fromEntries([...supportingPathCounts.entries()].slice(0, 12))
    };
  }

  function requestAbort(reason) {
    const normalized = sanitizeString(reason);
    if (!normalized || hardStopReason) {
      return;
    }

    hardStopReason = limitText(normalized, 240);
    writeRunLog("copilot.orchestration.abort", snapshot());
    queueMicrotask(() => {
      abortHandler?.(hardStopReason);
    });
  }

  function denyTool(toolName, toolArgs, reason) {
    const normalizedReason = limitText(reason, 240);
    const normalizedPath = normalizeRepositoryPath(
      pickObjectValue(toolArgs, ["path"]),
      input.workingDirectory
    );
    const alreadyRequestedStop = Boolean(stopRequestedReason);
    if (!stopRequestedReason) {
      stopRequestedReason = normalizedReason;
    } else {
      requestAbort(
        `GitHub Copilot kept requesting more tools after being told to stop. ${stopRequestedReason}`
      );
    }

    writeRunLog("copilot.tool.denied", {
      toolName,
      path: normalizedPath,
      reason: normalizedReason,
      alreadyRequestedStop,
      orchestration: snapshot()
    });

    return {
      permissionDecision: "deny",
      permissionDecisionReason: normalizedReason,
      modifiedArgs: toolArgs,
      additionalContext: buildCopilotStopUsingToolsContext(normalizedReason)
    };
  }

  function allowTool(toolArgs, additionalContext = null) {
    return {
      permissionDecision: "allow",
      modifiedArgs: toolArgs,
      ...(additionalContext ? { additionalContext } : {})
    };
  }

  function onPreToolUse(hookInput) {
    const toolName = sanitizeString(hookInput?.toolName) ?? "tool";
    const toolArgs = clampCopilotToolArgs(toolName, hookInput?.toolArgs);

    if (!COPILOT_TOUR_AVAILABLE_TOOLS.includes(toolName)) {
      return denyTool(
        toolName,
        toolArgs,
        `Only ${COPILOT_TOUR_AVAILABLE_TOOLS.join(", ")} are allowed while generating a code tour.`
      );
    }

    if (!COPILOT_TOUR_READ_TOOLS.has(toolName)) {
      return allowTool(toolArgs);
    }

    if (stopRequestedReason) {
      return denyTool(toolName, toolArgs, stopRequestedReason);
    }

    totalReadToolCalls += 1;

    const normalizedPath = normalizeRepositoryPath(
      pickObjectValue(toolArgs, ["path"]),
      input.workingDirectory
    );
    const isCandidatePath = normalizedPath ? candidatePaths.has(normalizedPath) : false;

    if (isCandidatePath) {
      inspectedCandidatePaths.add(normalizedPath);
    }

    let supportingPathCount = 0;
    if (normalizedPath && !isCandidatePath) {
      supportingPathCount = (supportingPathCounts.get(normalizedPath) ?? 0) + 1;
      supportingPathCounts.set(normalizedPath, supportingPathCount);
      totalSupportingToolCalls += 1;
    }

    if (totalReadToolCalls > maxReadToolCalls) {
      return denyTool(
        toolName,
        toolArgs,
        `GitHub Copilot already used ${maxReadToolCalls} read-only tool calls for this tour. Use the gathered evidence and return the final JSON now.`
      );
    }

    if (normalizedPath && !isCandidatePath && supportingPathCount > MAX_COPILOT_SUPPORTING_PATH_USES) {
      return denyTool(
        toolName,
        toolArgs,
        `Supporting path ${limitText(normalizedPath, 120)} was already inspected ${MAX_COPILOT_SUPPORTING_PATH_USES} times. Return the final JSON from the context you already gathered.`
      );
    }

    if (totalSupportingToolCalls > maxSupportingToolCalls) {
      return denyTool(
        toolName,
        toolArgs,
        `GitHub Copilot already used ${maxSupportingToolCalls} supporting lookups for this tour. Return the final JSON from the changed files and gathered context now.`
      );
    }

    if (
      !announcedCandidateCoverage &&
      candidatePaths.size > 0 &&
      inspectedCandidatePaths.size >= candidatePaths.size
    ) {
      announcedCandidateCoverage = true;
      return allowTool(
        toolArgs,
        "All changed files in the candidate tour have now been inspected. Only use a supporting callsite if it changes the walkthrough materially; otherwise return the final JSON now."
      );
    }

    return allowTool(toolArgs);
  }

  function onPostToolUse(hookInput) {
    const toolName = sanitizeString(hookInput?.toolName) ?? "tool";
    if (!COPILOT_TOUR_READ_TOOLS.has(toolName)) {
      return;
    }

    const normalizedPath = normalizeRepositoryPath(
      pickObjectValue(hookInput?.toolArgs, ["path"]),
      input.workingDirectory
    );
    if (!normalizedPath || candidatePaths.has(normalizedPath)) {
      return;
    }

    const supportingPathCount = supportingPathCounts.get(normalizedPath) ?? 0;
    if (supportingPathCount >= MAX_COPILOT_SUPPORTING_PATH_USES) {
      return {
        additionalContext: `Supporting path ${limitText(normalizedPath, 120)} has already been inspected enough to form the tour. Return the final JSON instead of reopening it.`
      };
    }
  }

  return {
    hooks: {
      onPreToolUse,
      onPostToolUse
    },
    registerAbortHandler(handler) {
      abortHandler = handler;
    },
    clearAbortHandler() {
      abortHandler = null;
    },
    hardStopReason() {
      return hardStopReason;
    },
    snapshot
  };
}

function computeCopilotReadToolBudget(candidateFileCount) {
  return Math.max(
    MIN_COPILOT_READ_TOOL_BUDGET,
    Math.min(MAX_COPILOT_READ_TOOL_BUDGET, candidateFileCount + COPILOT_READ_TOOL_BUDGET_BUFFER)
  );
}

function computeCopilotSupportingToolBudget(candidateFileCount) {
  return Math.max(
    MIN_COPILOT_SUPPORTING_TOOL_BUDGET,
    Math.min(
      MAX_COPILOT_SUPPORTING_TOOL_BUDGET,
      Math.ceil(candidateFileCount / 2) + 2
    )
  );
}

function clampCopilotToolArgs(toolName, toolArgs) {
  if (!toolArgs || typeof toolArgs !== "object" || Array.isArray(toolArgs)) {
    return toolArgs;
  }

  let modifiedArgs = toolArgs;

  if (toolName === "rg") {
    const headLimit = Number(pickObjectValue(toolArgs, ["head_limit", "headLimit"]));
    if (Number.isFinite(headLimit) && headLimit > MAX_COPILOT_RG_HEAD_LIMIT) {
      modifiedArgs = { ...modifiedArgs };
      if (Object.prototype.hasOwnProperty.call(toolArgs, "headLimit")) {
        modifiedArgs.headLimit = MAX_COPILOT_RG_HEAD_LIMIT;
      } else {
        modifiedArgs.head_limit = MAX_COPILOT_RG_HEAD_LIMIT;
      }
      writeRunLog("copilot.tool.args_clamped", {
        toolName,
        field: "head_limit",
        original: headLimit,
        clampedTo: MAX_COPILOT_RG_HEAD_LIMIT
      });
    }
  }

  if (toolName === "view") {
    const range = pickObjectValue(toolArgs, ["view_range", "viewRange"]);
    const clampedRange = clampViewRange(range);
    if (clampedRange) {
      const originalRange = JSON.stringify(range);
      const nextRange = JSON.stringify(clampedRange);
      if (originalRange !== nextRange) {
        if (modifiedArgs === toolArgs) {
          modifiedArgs = { ...modifiedArgs };
        }
        if (Object.prototype.hasOwnProperty.call(toolArgs, "viewRange")) {
          modifiedArgs.viewRange = clampedRange;
        } else {
          modifiedArgs.view_range = clampedRange;
        }
        writeRunLog("copilot.tool.args_clamped", {
          toolName,
          field: "view_range",
          original: range,
          clampedTo: clampedRange
        });
      }
    }
  }

  return modifiedArgs;
}

function clampViewRange(range) {
  if (!Array.isArray(range) || range.length === 0) {
    return null;
  }

  const start = Number.isFinite(range[0]) ? Math.max(1, Math.floor(range[0])) : null;
  if (start === null) {
    return null;
  }

  const end = Number.isFinite(range[1]) ? Math.floor(range[1]) : null;
  if (end === null) {
    return [start];
  }

  if (end < 0) {
    return [start, start + MAX_COPILOT_VIEW_RANGE_LINES - 1];
  }

  if (end - start + 1 > MAX_COPILOT_VIEW_RANGE_LINES) {
    return [start, start + MAX_COPILOT_VIEW_RANGE_LINES - 1];
  }

  return [start, end];
}

function buildCopilotStopUsingToolsContext(reason) {
  return `${reason} Use the changed files and previously gathered tool results to return the final code tour JSON now without requesting more tools.`;
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

function buildCopilotFallbackTour(input, model, error, orchestration = null) {
  const reason =
    orchestration?.hardStopReason() ??
    sanitizeString(error?.message) ??
    "GitHub Copilot did not finish a structured response.";
  writeRunLog("copilot.fallback.generated", {
    reason,
    orchestration: orchestration?.snapshot() ?? null
  });
  emitProgress({
    stage: "fallback",
    summary: "Using the verified pull-request context",
    detail:
      "GitHub Copilot did not finish a structured code tour, so the app is assembling a fallback walkthrough from the changed files and grouped review context.",
    log: "Using fallback code tour"
  });

  return mergeTour(
    {
      summary:
        "Fallback code tour assembled from the verified pull-request context and grouped changed files.",
      reviewFocus:
        "Review the grouped changed files and diff anchors below; GitHub Copilot did not finish a custom narrative for this run.",
      openQuestions: [],
      warnings: [limitText(reason, 240)],
      steps: [],
      sections: []
    },
    input,
    model
  );
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

  const preferredMatch = models
    .map((model, index) => ({
      id: sanitizeString(model?.id),
      index,
      score: scoreCopilotModel(model?.id)
    }))
    .filter((entry) => entry.id)
    .sort((left, right) => left.score - right.score || left.index - right.index)[0];

  return preferredMatch?.id ?? null;
}

function selectCopilotReasoningEffort(models, modelId) {
  const normalizedModelId = sanitizeString(modelId);
  if (!normalizedModelId) {
    return null;
  }

  const selectedModel = ensureArray(models).find(
    (candidate) => sanitizeString(candidate?.id) === normalizedModelId
  );
  if (!selectedModel?.capabilities?.supports?.reasoningEffort) {
    return null;
  }

  const supportedReasoningEfforts = ensureArray(
    selectedModel?.supportedReasoningEfforts
  )
    .map((value) => sanitizeString(value)?.toLowerCase())
    .filter(Boolean);
  if (supportedReasoningEfforts.includes("medium")) {
    return "medium";
  }

  const defaultReasoningEffort = sanitizeString(
    selectedModel?.defaultReasoningEffort
  )?.toLowerCase();
  return defaultReasoningEffort ?? supportedReasoningEfforts[0] ?? null;
}

function scoreCopilotModel(modelId) {
  const normalized = sanitizeString(modelId)?.toLowerCase();
  if (!normalized) {
    return Number.MAX_SAFE_INTEGER;
  }

  if (normalized === "gpt-5.4") {
    return 0;
  }
  if (normalized.startsWith("gpt-5") && normalized.includes("mini")) {
    return 1;
  }
  if (normalized.includes("mini")) {
    return 2;
  }
  if (normalized.includes("haiku")) {
    return 3;
  }
  if (normalized === "gpt-5") {
    return 4;
  }
  if (normalized.startsWith("gpt-5")) {
    return 5;
  }
  if (normalized.startsWith("gpt-4.1")) {
    return 6;
  }
  if (normalized.startsWith("claude")) {
    return 7;
  }
  if (normalized.startsWith("o")) {
    return 8;
  }

  return 100;
}

async function waitForCopilotTurn(session, prompt, workingDirectory, orchestration = null) {
  const activeTools = new Map();
  const requestedTools = new Map();
  let finalMessage = null;
  let streamedResponseStarted = false;
  let settled = false;
  let reasoningDeltaCount = 0;
  let usageCount = 0;
  let lastReasoningHeartbeatAt = 0;
  let resolveDone = null;
  let rejectDone = null;
  const done = new Promise((resolve, reject) => {
    resolveDone = resolve;
    rejectDone = reject;
  });
  const resolveOnce = (value) => {
    if (settled) {
      return;
    }
    settled = true;
    writeRunLog("copilot.turn.resolved", {
      hasFinalMessage: Boolean(value),
      streamedResponseStarted,
      reasoningDeltaCount,
      usageCount
    });
    resolveDone(value);
  };
  const rejectOnce = (error) => {
    if (settled) {
      return;
    }
    settled = true;
    writeRunLog("copilot.turn.rejected", {
      message: error instanceof Error ? error.message : String(error),
      streamedResponseStarted,
      reasoningDeltaCount,
      usageCount
    });
    rejectDone(error);
  };
  const watchdog = createGenerationWatchdog((abortReason) => {
    writeRunLog("copilot.watchdog.abort", abortReason);
    emitProgress({
      stage: "timeout",
      summary: generationAbortMessage("GitHub Copilot", abortReason),
      detail:
        "Aborting the Copilot run so the app can surface the failure without waiting for the old 10 minute timeout.",
      log: generationAbortMessage("GitHub Copilot", abortReason)
    });
    session.abort().catch(() => {});
    rejectOnce(new Error(generationAbortMessage("GitHub Copilot", abortReason)));
  });
  orchestration?.registerAbortHandler((reason) => {
    emitProgress({
      stage: "orchestration_limit",
      summary: "Stopping repetitive tool use",
      detail:
        "GitHub Copilot kept requesting more tools after the tour already had enough verified context.",
      log: limitText(reason, 240)
    });
    session.abort().catch(() => {});
    rejectOnce(new Error(reason));
  });
  const subscriptions = [
    session.on("assistant.message", (event) => {
      finalMessage = event;
      watchdog.touchVisible("assistant.message");
      writeRunLog("copilot.assistant.message", {
        hasContent: Boolean(sanitizeString(event?.data?.content)),
        toolRequests: ensureArray(event?.data?.toolRequests).map((toolRequest) => ({
          toolCallId: sanitizeString(toolRequest?.toolCallId),
          name: sanitizeString(toolRequest?.name),
          intentionSummary: sanitizeString(toolRequest?.intentionSummary)
        })),
        contentPreview: sanitizeOptionalString(event?.data?.content, 800)
      });
      for (const toolRequest of ensureArray(event?.data?.toolRequests)) {
        requestedTools.set(toolRequest.toolCallId, toolRequest);
      }
    }),
    session.on("assistant.message_delta", () => {
      watchdog.touchVisible("assistant.message_delta");
      if (streamedResponseStarted) {
        return;
      }
      streamedResponseStarted = true;
      writeRunLog("copilot.assistant.message_delta.started");
      emitProgress({
        stage: "drafting",
        summary: "GitHub Copilot is streaming a response",
        detail: "Receiving incremental output from the current Copilot turn.",
        log: "Copilot started streaming a response"
      });
    }),
    session.on("assistant.reasoning_delta", (event) => {
      watchdog.touchBackground();
      reasoningDeltaCount += 1;
      const now = Date.now();
      if (reasoningDeltaCount <= 3 || now - lastReasoningHeartbeatAt >= 10_000) {
        lastReasoningHeartbeatAt = now;
        writeRunLog("copilot.assistant.reasoning", {
          count: reasoningDeltaCount,
          detail: snapshotForLog(event?.data, 1_200)
        });
      }
    }),
    session.on("tool.execution_start", (event) => {
      const requestInfo = requestedTools.get(event.data.toolCallId) ?? null;
      requestedTools.delete(event.data.toolCallId);
      const toolContext = describeCopilotTool(
        event.data,
        requestInfo,
        workingDirectory
      );
      watchdog.touchVisible(toolContext?.log ?? "tool.execution_start");
      writeRunLog("copilot.tool.start", {
        toolCallId: sanitizeString(event?.data?.toolCallId),
        tool: toolContext?.log ?? null,
        request: snapshotForLog(requestInfo, 2_000),
        event: snapshotForLog(event?.data, 2_000)
      });
      activeTools.set(event.data.toolCallId, toolContext);
      const progress = progressForCopilotToolStart(toolContext);
      if (progress) {
        emitProgress(progress);
      }
    }),
    session.on("tool.execution_progress", (event) => {
      const toolContext = activeTools.get(event.data.toolCallId);
      watchdog.touchVisible(toolContext?.log ?? "tool.execution_progress");
      const progress = progressForCopilotToolProgress(
        event.data,
        toolContext
      );
      if (progress?.detail) {
        writeRunLog("copilot.tool.progress", {
          toolCallId: sanitizeString(event?.data?.toolCallId),
          tool: toolContext?.log ?? null,
          progress: progress.detail
        });
      }
      if (progress) {
        emitProgress(progress);
      }
    }),
    session.on("tool.execution_complete", (event) => {
      const toolContext = activeTools.get(event.data.toolCallId);
      watchdog.touchVisible(toolContext?.log ?? "tool.execution_complete");
      writeRunLog("copilot.tool.complete", {
        toolCallId: sanitizeString(event?.data?.toolCallId),
        tool: toolContext?.log ?? null,
        success: event?.data?.success,
        event: snapshotForLog(event?.data, 2_000)
      });
      activeTools.delete(event.data.toolCallId);

      if (!event.data.success) {
        const progress = progressForCopilotToolFailure(event.data, toolContext);
        if (progress) {
          emitProgress(progress);
        }
      }
    }),
    session.on("subagent.started", (event) => {
      watchdog.touchVisible("subagent.started");
      writeRunLog("copilot.subagent.started", snapshotForLog(event?.data, 1_500));
      emitProgress(progressForSubagentStart(event.data));
    }),
    session.on("subagent.completed", (event) => {
      watchdog.touchVisible("subagent.completed");
      writeRunLog("copilot.subagent.completed", snapshotForLog(event?.data, 1_500));
      emitProgress(progressForSubagentComplete(event.data));
    }),
    session.on("subagent.failed", (event) => {
      watchdog.touchVisible("subagent.failed");
      writeRunLog("copilot.subagent.failed", snapshotForLog(event?.data, 1_500));
      emitProgress(progressForSubagentFailure(event.data));
    }),
    session.on("assistant.usage", (event) => {
      watchdog.touchBackground();
      usageCount += 1;
      if (usageCount <= 3 || usageCount % 10 === 0) {
        writeRunLog("copilot.assistant.usage", {
          count: usageCount,
          detail: snapshotForLog(event?.data, 1_000)
        });
      }
    }),
    session.on("session.error", (event) => {
      watchdog.touchVisible("session.error");
      writeRunLog("copilot.session.error", snapshotForLog(event?.data, 2_000));
      rejectOnce(new Error(event.data.message));
    }),
    session.on("session.idle", () => {
      writeRunLog("copilot.session.idle", {
        hasFinalMessage: Boolean(finalMessage),
        activeToolCount: activeTools.size,
        requestedToolCount: requestedTools.size
      });
      if (watchdog.abortReason()) {
        rejectOnce(
          new Error(generationAbortMessage("GitHub Copilot", watchdog.abortReason()))
        );
        return;
      }

      if (!finalMessage) {
        rejectOnce(
          new Error(
            "GitHub Copilot finished the turn without returning the structured code tour."
          )
        );
        return;
      }

      resolveOnce(finalMessage);
    })
  ];

  try {
    trackProgress(watchdog, {
      stage: "running",
      summary: "GitHub Copilot is inspecting the checkout",
      detail:
        "Waiting for live tool activity or streamed output from the current tour run.",
      log: "Inspecting the linked checkout"
    });
    writeRunLog("copilot.session.send", {
      promptLength: prompt.length
    });
    await withTimeout(
      session.send({ prompt }),
      STATUS_TIMEOUT_MS,
      "Timed out while sending the code tour request to GitHub Copilot."
    );
    writeRunLog("copilot.session.send.accepted");
    return await done;
  } finally {
    orchestration?.clearAbortHandler();
    watchdog.clear();
    for (const unsubscribe of subscriptions) {
      unsubscribe();
    }
  }
}

function createGenerationWatchdog(onAbort) {
  let abortReason = null;
  let overallTimer = null;
  let inactivityTimer = null;
  let lastVisibleActivity = "starting request";
  const abortWith = (kind, timeoutMs) => {
    if (abortReason) {
      return;
    }

    abortReason = { kind, timeoutMs, lastVisibleActivity };
    onAbort?.(abortReason);
  };
  const resetInactivity = () => {
    if (abortReason) {
      return;
    }

    if (inactivityTimer !== null) {
      clearTimeout(inactivityTimer);
    }
    inactivityTimer = setTimeout(() => {
      abortWith("inactivity", GENERATION_INACTIVITY_TIMEOUT_MS);
    }, GENERATION_INACTIVITY_TIMEOUT_MS);
  };

  overallTimer = setTimeout(() => {
    abortWith("overall", GENERATION_TIMEOUT_MS);
  }, GENERATION_TIMEOUT_MS);
  resetInactivity();

  return {
    touch() {
      this.touchVisible(null);
    },
    touchVisible(activity) {
      const normalized = sanitizeString(activity);
      if (normalized) {
        lastVisibleActivity = limitText(normalized, 160);
      }
      resetInactivity();
    },
    touchBackground() {
      resetInactivity();
    },
    clear() {
      if (overallTimer !== null) {
        clearTimeout(overallTimer);
      }
      if (inactivityTimer !== null) {
        clearTimeout(inactivityTimer);
      }
    },
    abortReason() {
      return abortReason;
    },
    lastVisibleActivity() {
      return lastVisibleActivity;
    }
  };
}

function trackProgress(watchdog, progress) {
  watchdog.touchVisible(progress?.log ?? progress?.summary ?? null);
  if (progress) {
    emitProgress(progress);
  }
}

function emitProgress({ stage, summary, detail = null, log = null }) {
  const payload = {
    stage,
    summary: limitText(summary, 160),
    detail: detail ? limitText(detail, 240) : null,
    log: log ? limitText(log, 240) : null,
    logFilePath: currentRunLogger?.filePath ?? null
  };
  writeRunLog("progress", payload);
  process.stderr.write(
    `${PROGRESS_PREFIX}${JSON.stringify(payload)}\n`
  );
}

function generationAbortMessage(providerLabel, abortReason) {
  if (!abortReason) {
    return `${providerLabel} stopped while generating the code tour.`;
  }

  const lastVisibleActivity = sanitizeString(abortReason.lastVisibleActivity);
  const lastVisibleSuffix = lastVisibleActivity
    ? ` Last visible activity: ${lastVisibleActivity}.`
    : "";

  if (abortReason.kind === "inactivity") {
    return `${providerLabel} stopped reporting progress for ${formatDuration(
      abortReason.timeoutMs
    )} while generating the code tour.${lastVisibleSuffix}`;
  }

  return `${providerLabel} timed out while generating the code tour after ${formatDuration(
    abortReason.timeoutMs
  )}.${lastVisibleSuffix}`;
}

function formatDuration(timeoutMs) {
  const totalSeconds = Math.round(timeoutMs / 1000);
  if (totalSeconds % 60 === 0) {
    const minutes = totalSeconds / 60;
    return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  }

  return `${totalSeconds} second${totalSeconds === 1 ? "" : "s"}`;
}

function progressForCopilotToolStart(toolContext) {
  if (!toolContext || toolContext.hidden) {
    return null;
  }

  return {
    stage: "tool",
    summary: toolContext.summary,
    detail: toolContext.detail,
    log: toolContext.log
  };
}

function progressForCopilotToolProgress(data, toolContext) {
  if (!toolContext || toolContext.hidden) {
    return null;
  }

  const progressMessage = sanitizeString(data.progressMessage);
  if (!progressMessage) {
    return null;
  }

  return {
    stage: "tool_progress",
    summary: toolContext.summary,
    detail: progressMessage,
    log: `${toolContext.log}: ${limitText(progressMessage, 120)}`
  };
}

function progressForCopilotToolFailure(data, toolContext) {
  if (toolContext?.suppressFailure) {
    return null;
  }

  const toolLabel = toolContext?.log ?? describeToolName(data?.toolName, null);
  const detail =
    sanitizeString(data.error?.message) ??
    `${toolLabel} failed before Copilot could finish the tour.`;

  return {
    stage: "tool_failed",
    summary: `${toolLabel} failed`,
    detail,
    log: `${toolLabel} failed`
  };
}

function progressForSubagentStart(data) {
  return {
    stage: "subagent",
    summary: `GitHub Copilot spawned ${data.agentDisplayName}`,
    detail:
      sanitizeOptionalString(data.agentDescription, 240) ??
      "Delegating a focused part of the review to a sub-agent.",
    log: `Sub-agent started: ${data.agentDisplayName}`
  };
}

function progressForSubagentComplete(data) {
  const detailParts = [];
  if (typeof data.totalToolCalls === "number") {
    detailParts.push(`${data.totalToolCalls} tool call${data.totalToolCalls === 1 ? "" : "s"}`);
  }
  if (typeof data.durationMs === "number") {
    detailParts.push(`finished in ${formatDuration(data.durationMs)}`);
  }

  return {
    stage: "subagent_complete",
    summary: `${data.agentDisplayName} finished`,
    detail:
      detailParts.length > 0
        ? detailParts.join(" • ")
        : "Returning the sub-agent result to the main Copilot run.",
    log: `Sub-agent finished: ${data.agentDisplayName}`
  };
}

function progressForSubagentFailure(data) {
  return {
    stage: "subagent_failed",
    summary: `${data.agentDisplayName} failed`,
    detail: sanitizeOptionalString(data.error, 240) ?? "The sub-agent failed.",
    log: `Sub-agent failed: ${data.agentDisplayName}`
  };
}

function progressForCodexEvent(event) {
  switch (event.type) {
    case "thread.started":
      return {
        stage: "thread",
        summary: "Codex started a new thread",
        detail: "The agent is ready to inspect the linked checkout.",
        log: "Started Codex thread"
      };
    case "turn.started":
      return {
        stage: "turn",
        summary: "Codex is inspecting the change",
        detail: "Walking the changed files and related callsites from the checkout.",
        log: "Inspecting the changed files"
      };
    case "item.started":
      return progressForCodexItem(event.item, "started");
    case "item.updated":
      return event.item.type === "todo_list"
        ? progressForCodexItem(event.item, "updated")
        : null;
    case "item.completed":
      return progressForCodexItem(event.item, "completed");
    case "turn.completed":
      return {
        stage: "finalizing",
        summary: "Codex finished gathering context",
        detail: "Formatting the structured code tour response.",
        log: "Codex finished its turn"
      };
    default:
      return null;
  }
}

function progressForCodexItem(item, status) {
  switch (item.type) {
    case "command_execution":
      if (status === "started") {
        return {
          stage: "command",
          summary: "Codex is running a checkout command",
          detail: sanitizeOptionalString(item.command, 240),
          log: `Command: ${limitText(item.command, 160)}`
        };
      }

      if (status === "completed" && item.status === "failed") {
        return {
          stage: "command_failed",
          summary: "A Codex command failed",
          detail: sanitizeOptionalString(item.command, 240),
          log: `Command failed: ${limitText(item.command, 160)}`
        };
      }

      return null;
    case "mcp_tool_call": {
      const toolRef = `${item.server}/${item.tool}`;
      if (status === "started") {
        return {
          stage: "tool",
          summary: "Codex is using a tool",
          detail: toolRef,
          log: `Tool: ${toolRef}`
        };
      }

      if (status === "completed" && item.status === "failed") {
        return {
          stage: "tool_failed",
          summary: "A Codex tool step failed",
          detail:
            sanitizeOptionalString(item.error?.message, 240) ?? `Tool failed: ${toolRef}`,
          log: `Tool failed: ${toolRef}`
        };
      }

      return null;
    }
    case "todo_list": {
      const nextTodo = ensureArray(item.items).find((entry) => !entry.completed)?.text;
      const detail =
        sanitizeOptionalString(nextTodo, 240) ??
        "Updating the current plan for the code tour run.";

      return {
        stage: "planning",
        summary: "Codex is updating its review plan",
        detail,
        log: detail
      };
    }
    case "reasoning":
      return {
        stage: "reasoning",
        summary: "Codex is reasoning through the change",
        detail:
          sanitizeOptionalString(item.text, 240) ??
          "Working through the current reasoning step.",
        log:
          sanitizeOptionalString(item.text, 180) ??
          "Codex updated its reasoning"
      };
    case "web_search":
      return {
        stage: "search",
        summary: "Codex is searching for context",
        detail:
          sanitizeOptionalString(item.query, 240) ?? "Issuing a search query.",
        log:
          sanitizeOptionalString(item.query, 180) ?? "Codex issued a search query"
      };
    case "agent_message":
      if (status === "completed") {
        return {
          stage: "drafting",
          summary: "Codex drafted the code tour response",
          detail: "Finalizing the structured output for the app.",
          log: "Codex drafted the final response"
        };
      }

      return null;
    default:
      return null;
  }
}

function describeToolName(primaryName, fallbackName) {
  const candidate = sanitizeString(primaryName) ?? sanitizeString(fallbackName);
  if (!candidate) {
    return "tool";
  }

  const humanized = candidate.replace(/[._-]+/g, " ").replace(/\s+/g, " ").trim();
  return humanized.length > 0 ? humanized : "tool";
}

function describeCopilotTool(data, requestInfo, workingDirectory) {
  const toolName =
    sanitizeString(requestInfo?.name) ??
    sanitizeString(data?.toolName) ??
    sanitizeString(data?.mcpToolName) ??
    "tool";
  const toolArgs = requestInfo?.arguments ?? data?.arguments ?? {};
  const toolTitle = sanitizeString(requestInfo?.toolTitle);
  const intentionSummary = sanitizeString(requestInfo?.intentionSummary);
  const fallbackLabel =
    intentionSummary ??
    toolTitle ??
    describeToolName(toolName, data?.mcpToolName);

  switch (toolName) {
    case "view":
      return {
        summary: "GitHub Copilot is reading code",
        detail: describeViewActivity(toolArgs, workingDirectory) ?? fallbackLabel,
        log: describeViewLog(toolArgs, workingDirectory) ?? "Read file",
        suppressFailure: true,
        hidden: false
      };
    case "rg":
      return {
        summary: "GitHub Copilot is searching the checkout",
        detail: describeRipgrepActivity(toolArgs, workingDirectory) ?? fallbackLabel,
        log: describeRipgrepLog(toolArgs, workingDirectory) ?? "Search checkout",
        suppressFailure: true,
        hidden: false
      };
    case "glob":
      return {
        summary: "GitHub Copilot is listing matching files",
        detail: describeGlobActivity(toolArgs, workingDirectory) ?? fallbackLabel,
        log: describeGlobLog(toolArgs, workingDirectory) ?? "List matching files",
        suppressFailure: true,
        hidden: false
      };
    case "bash":
      return {
        summary: "GitHub Copilot is running a shell command",
        detail: describeBashActivity(toolArgs) ?? fallbackLabel,
        log: describeBashLog(toolArgs) ?? "Run shell command",
        suppressFailure: false,
        hidden: false
      };
    case "report_intent":
      return {
        summary: "GitHub Copilot is planning the next step",
        detail: sanitizeString(toolArgs.intent) ?? fallbackLabel,
        log: sanitizeString(toolArgs.intent) ?? "Update plan",
        suppressFailure: true,
        hidden: false
      };
    default:
      if (toolName.startsWith("github-mcp-server-")) {
        return {
          summary: "GitHub Copilot is querying GitHub",
          detail:
            describeGitHubToolActivity(toolName, toolArgs, workingDirectory) ??
            fallbackLabel,
          log:
            describeGitHubToolLog(toolName, toolArgs, workingDirectory) ??
            fallbackLabel,
          suppressFailure: false,
          hidden: false
        };
      }

      return {
        summary: "GitHub Copilot is running a tool",
        detail: fallbackLabel,
        log: fallbackLabel,
        suppressFailure: false,
        hidden: false
      };
  }
}

function describeViewActivity(toolArgs, workingDirectory) {
  const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);
  if (!path) {
    return null;
  }

  const range = formatViewRange(
    pickObjectValue(toolArgs, ["view_range", "viewRange"])
  );
  return range ? `Reading ${path}${range}.` : `Reading ${path}.`;
}

function describeViewLog(toolArgs, workingDirectory) {
  const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);
  if (!path) {
    return null;
  }

  const range = formatViewRange(
    pickObjectValue(toolArgs, ["view_range", "viewRange"])
  );
  return range ? `Read ${path}${range}` : `Read ${path}`;
}

function describeRipgrepActivity(toolArgs, workingDirectory) {
  const pattern = formatInlineCode(pickObjectValue(toolArgs, ["pattern"]));
  const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);
  const globPattern = formatInlineCode(pickObjectValue(toolArgs, ["glob"]));

  if (pattern && path && globPattern) {
    return `Searching ${path} for ${pattern} in ${globPattern} files.`;
  }
  if (pattern && path) {
    return `Searching ${path} for ${pattern}.`;
  }
  if (pattern) {
    return `Searching the checkout for ${pattern}.`;
  }

  return path ? `Searching ${path}.` : null;
}

function describeRipgrepLog(toolArgs, workingDirectory) {
  const pattern = formatInlineCode(pickObjectValue(toolArgs, ["pattern"]));
  const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);

  if (pattern && path) {
    return `Search ${path} for ${pattern}`;
  }
  if (pattern) {
    return `Search for ${pattern}`;
  }

  return path ? `Search ${path}` : null;
}

function describeGlobActivity(toolArgs, workingDirectory) {
  const pattern = formatInlineCode(pickObjectValue(toolArgs, ["pattern"]));
  const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);

  if (pattern && path) {
    return `Listing files in ${path} matching ${pattern}.`;
  }
  if (pattern) {
    return `Listing files matching ${pattern}.`;
  }

  return path ? `Listing files in ${path}.` : null;
}

function describeGlobLog(toolArgs, workingDirectory) {
  const pattern = formatInlineCode(pickObjectValue(toolArgs, ["pattern"]));
  const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);

  if (pattern && path) {
    return `List ${pattern} in ${path}`;
  }
  if (pattern) {
    return `List ${pattern}`;
  }

  return path ? `List files in ${path}` : null;
}

function describeBashActivity(toolArgs) {
  return (
    sanitizeString(pickObjectValue(toolArgs, ["description"])) ??
    sanitizeOptionalString(pickObjectValue(toolArgs, ["command"]), 240)
  );
}

function describeBashLog(toolArgs) {
  return (
    sanitizeOptionalString(pickObjectValue(toolArgs, ["description"]), 160) ??
    sanitizeOptionalString(pickObjectValue(toolArgs, ["command"]), 160)
  );
}

function describeGitHubToolActivity(toolName, toolArgs, workingDirectory) {
  if (toolName === "github-mcp-server-get_file_contents") {
    const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);
    const repo = formatRepository(toolArgs);
    if (path && repo) {
      return `Reading ${path} from ${repo}.`;
    }
    if (path) {
      return `Reading ${path} from GitHub.`;
    }
  }

  const method = sanitizeString(pickObjectValue(toolArgs, ["method"]));
  const repo = formatRepository(toolArgs);
  const resourceId = sanitizeString(
    pickObjectValue(toolArgs, ["resource_id", "resourceId"])
  );

  if (method && repo && resourceId) {
    return `Running ${method} for ${repo} (${resourceId}).`;
  }
  if (method && repo) {
    return `Running ${method} for ${repo}.`;
  }
  if (repo) {
    return `Querying ${repo} on GitHub.`;
  }

  return null;
}

function describeGitHubToolLog(toolName, toolArgs, workingDirectory) {
  if (toolName === "github-mcp-server-get_file_contents") {
    const path = formatToolPath(pickObjectValue(toolArgs, ["path"]), workingDirectory);
    const repo = formatRepository(toolArgs);
    if (path && repo) {
      return `Read ${path} from ${repo}`;
    }
    if (path) {
      return `Read ${path} from GitHub`;
    }
  }

  const method = sanitizeString(pickObjectValue(toolArgs, ["method"]));
  const repo = formatRepository(toolArgs);
  if (method && repo) {
    return `GitHub ${method} on ${repo}`;
  }
  if (repo) {
    return `Query GitHub for ${repo}`;
  }

  return null;
}

function formatToolPath(value, workingDirectory) {
  const path = sanitizeString(value);
  if (!path) {
    return null;
  }

  if (workingDirectory) {
    try {
      const relativePath = relative(workingDirectory, path);
      if (relativePath === "") {
        return ".";
      }
      if (!relativePath.startsWith("..")) {
        return relativePath;
      }
    } catch {}
  }

  return path;
}

function normalizeRepositoryPath(value, workingDirectory) {
  const normalized = formatToolPath(value, workingDirectory);
  if (!normalized) {
    return null;
  }

  return normalized.replace(/\\/g, "/").replace(/^\.\/+/, "");
}

function formatViewRange(value) {
  if (!Array.isArray(value) || value.length === 0) {
    return "";
  }

  const start = Number.isFinite(value[0]) ? Math.max(1, Math.floor(value[0])) : null;
  const end = Number.isFinite(value[1]) ? Math.floor(value[1]) : null;
  if (start === null) {
    return "";
  }
  if (end === null || end === start) {
    return `:${start}`;
  }
  if (end < 0) {
    return `:${start}+`;
  }
  return `:${start}-${Math.max(start, end)}`;
}

function formatInlineCode(value) {
  const text = sanitizeString(value);
  return text ? `\`${limitText(text, 80)}\`` : null;
}

function pickObjectValue(value, keys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return null;
  }

  for (const key of keys) {
    if (key in value) {
      return value[key];
    }
  }

  return null;
}

function formatRepository(toolArgs) {
  const owner = sanitizeString(pickObjectValue(toolArgs, ["owner"]));
  const repo = sanitizeString(pickObjectValue(toolArgs, ["repo"]));
  if (!owner || !repo) {
    return null;
  }

  return `${owner}/${repo}`;
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
  const overrideCandidate = resolveExplicitCliPath(name);
  if (overrideCandidate) {
    return overrideCandidate;
  }

  if (PROJECT_ROOT) {
    const localCandidate = resolve(PROJECT_ROOT, "node_modules", ".bin", name);
    if (hasExecutablePath(localCandidate)) {
      return localCandidate;
    }
  }

  return findBinaryOnPath(name);
}

function createRunLoggerForRequest(request) {
  if (sanitizeString(request?.action) !== "generate") {
    return null;
  }

  const directory = resolveCodeTourLogDirectory();

  try {
    mkdirSync(directory, { recursive: true });
  } catch {
    return null;
  }

  const filePath = join(directory, buildRunLogFileName(request?.context));

  try {
    appendFileSync(filePath, "");
    return { filePath };
  } catch {
    return null;
  }
}

function resolveCodeTourLogDirectory() {
  const configured = sanitizeString(process.env[CODE_TOUR_LOG_DIR_ENV]);
  if (configured) {
    return configured;
  }

  return resolve(tmpdir(), "gh-ui-tool", "logs", "code-tours");
}

function buildRunLogFileName(input) {
  const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
  const provider = sanitizeLogFileComponent(input?.provider ?? "provider");
  const repository = sanitizeLogFileComponent(input?.repository ?? "repository");
  const prNumber =
    typeof input?.number === "number" && Number.isFinite(input.number)
      ? `pr${Math.max(0, Math.floor(input.number))}`
      : "pr";

  return `${timestamp}-${provider}-${repository}-${prNumber}-${process.pid}.log`;
}

function sanitizeLogFileComponent(value) {
  const normalized = sanitizeString(String(value)) ?? "unknown";
  const sanitized = normalized
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "");

  return sanitized.length > 0 ? sanitized : "unknown";
}

function summarizeRequestForLog(request) {
  if (sanitizeString(request?.action) !== "generate") {
    return { action: sanitizeString(request?.action) ?? "unknown" };
  }

  const input = request?.context ?? {};
  return {
    action: "generate",
    provider: sanitizeString(input.provider),
    repository: sanitizeString(input.repository),
    number:
      typeof input.number === "number" && Number.isFinite(input.number)
        ? Math.floor(input.number)
        : null,
    workingDirectory: sanitizeString(input.workingDirectory),
    candidateStepCount: ensureArray(input.candidateSteps).length,
    candidateGroupCount: ensureArray(input.candidateGroups).length,
    fileCount: ensureArray(input.files).length,
    reviewThreadCount: ensureArray(input.reviewThreads).length
  };
}

function emitDebugLogLocation() {
  if (!currentRunLogger?.filePath) {
    return;
  }

  emitProgress({
    stage: "logging",
    summary: "Writing code tour debug log",
    detail: `Saving detailed provider activity to ${currentRunLogger.filePath}.`,
    log: `Writing debug log to ${currentRunLogger.filePath}`
  });
}

function formatErrorMessageWithLogPath(message) {
  if (!currentRunLogger?.filePath) {
    return message;
  }

  return `${message} Debug log: ${currentRunLogger.filePath}`;
}

function writeRunLog(event, data = null) {
  if (!currentRunLogger?.filePath) {
    return;
  }

  const suffix = data === null || data === undefined ? "" : ` ${snapshotForLog(data, 8_000)}`;

  try {
    appendFileSync(
      currentRunLogger.filePath,
      `${new Date().toISOString()} ${event}${suffix}\n`
    );
  } catch {}
}

function snapshotForLog(value, maxLength = 4_000) {
  if (value === null || value === undefined) {
    return "null";
  }

  if (typeof value === "string") {
    return JSON.stringify(limitText(value, maxLength));
  }

  try {
    return limitText(JSON.stringify(value, logJsonReplacer), maxLength);
  } catch {
    return JSON.stringify(limitText(String(value), maxLength));
  }
}

function logJsonReplacer(_key, value) {
  if (typeof value === "bigint") {
    return value.toString();
  }
  if (value instanceof Error) {
    return {
      name: value.name,
      message: value.message,
      stack: value.stack
    };
  }
  if (typeof value === "string") {
    return limitText(value, 2_000);
  }

  return value;
}

function resolveProjectRoot() {
  const candidate = sanitizeString(process.env.GH_UI_CODE_TOUR_PROJECT_ROOT);
  if (!candidate) {
    return null;
  }

  return existsSync(candidate) ? candidate : null;
}

function resolveExplicitCliPath(name) {
  const envName =
    name === "codex" ? "GH_UI_TOOL_CODEX_BINARY" : "GH_UI_TOOL_COPILOT_BINARY";
  const candidate = sanitizeString(process.env[envName]);

  if (!candidate) {
    return null;
  }

  return hasExecutablePath(candidate) ? candidate : null;
}

function findBinaryOnPath(name) {
  const rawPath = sanitizeString(process.env.PATH);
  if (!rawPath) {
    return null;
  }

  for (const segment of rawPath.split(delimiter)) {
    const trimmed = segment.trim();
    if (!trimmed) {
      continue;
    }

    const candidate = resolve(trimmed, name);
    if (hasExecutablePath(candidate)) {
      return candidate;
    }
  }

  return null;
}

function hasExecutablePath(candidate) {
  if (!candidate || !existsSync(candidate)) {
    return false;
  }

  try {
    accessSync(candidate, constants.X_OK);
    return true;
  } catch {
    return false;
  }
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

  const candidates = buildStructuredResponseParseCandidates(raw);

  if (candidates.length === 0) {
    throw new Error("The LLM response did not contain a valid JSON object.");
  }

  let parseError = null;

  for (const candidate of candidates) {
    const parsed = tryParseJson(candidate.value);

    if (parsed.ok) {
      return parsed.value;
    }

    parseError = parsed.error;
  }

  let repairError = null;

  for (const candidate of candidates) {
    const repaired = tryRepairJson(candidate.value);

    if (repaired.ok) {
      writeRunLog("llm.response.repaired", {
        candidate: candidate.label,
        originalLength: candidate.value.length,
        repairedLength: repaired.repaired.length,
        originalPreview: limitText(candidate.value, 1_000),
        repairedPreview: limitText(repaired.repaired, 1_000)
      });
      return repaired.value;
    }

    repairError = repaired.error;
  }

  throw new Error(
    `Failed to parse the LLM response as JSON: ${
      parseError instanceof Error ? parseError.message : "unknown error"
    }${
      repairError instanceof Error
        ? ` (repair attempt failed: ${repairError.message})`
        : ""
    }`
  );
}

function buildStructuredResponseParseCandidates(raw) {
  const trimmed = raw.trim();
  const unwrapped = unwrapMarkdownCodeFence(trimmed);
  const extractedJson = extractFirstJsonObject(unwrapped);
  const candidates = [
    { label: "raw", value: trimmed },
    { label: "unwrapped", value: unwrapped },
    extractedJson ? { label: "extracted_object", value: extractedJson } : null
  ].filter(Boolean);

  return candidates.filter(
    (candidate, index) =>
      candidate.value &&
      candidates.findIndex((entry) => entry.value === candidate.value) === index
  );
}

function unwrapMarkdownCodeFence(value) {
  const trimmed = value.trim();
  const match = trimmed.match(/^```(?:json)?\s*([\s\S]*?)\s*```$/i);
  return match ? match[1].trim() : trimmed;
}

function tryParseJson(value) {
  try {
    return { ok: true, value: JSON.parse(value) };
  } catch (error) {
    return { ok: false, error };
  }
}

function tryRepairJson(value) {
  try {
    const repaired = jsonrepair(value);
    return {
      ok: true,
      repaired,
      value: JSON.parse(repaired)
    };
  } catch (error) {
    return { ok: false, error };
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
