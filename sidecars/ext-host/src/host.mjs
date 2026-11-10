import path from "node:path";
import { pathToFileURL } from "node:url";

import { createCommandRegistry } from "./commands.mjs";
import { createPluginStorage } from "./storage.mjs";
import { createToolRegistry } from "./tools.mjs";
import { createWorkspace, isPathInside } from "./workspace.mjs";
import { RPC_INVALID_PARAMS, RPC_INVALID_REQUEST, RPC_INTERNAL_ERROR, RPC_METHOD_NOT_FOUND, RPC_PARSE_ERROR, errorResponse, resultResponse, serializableResult, thrownDetails } from "./jsonrpc.mjs";

const DEACTIVATE_BUDGET_MS = 1500;
const STDOUT_FLUSH_GRACE_MS = 500;

const stderrLog = console.error.bind(console);
const commandRegistry = createCommandRegistry();
const toolRegistry = createToolRegistry();

let loadedPlugins = [];
let initialized = false;
let shuttingDown = false;
let deactivated = false;
let shutdownPromise = null;

function hostLog(...args) {
  stderrLog("[ext-host]", ...args);
}

function pluginLog(pluginId, ...args) {
  stderrLog(`[ext-host:${pluginId}]`, ...args);
}

function messageOf(error) {
  return error instanceof Error ? error.message : String(error);
}

function pluginError(stage, message) {
  const error = new Error(message);
  error.pluginStage = stage;
  return error;
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function reserveStdout() {
  const toStderr = (...args) => stderrLog(...args);
  console.log = toStderr;
  console.info = toStderr;
  console.debug = toStderr;
}

function writeResponse(response) {
  return new Promise((resolve) => {
    let line;
    try {
      line = `${JSON.stringify(response)}\n`;
    } catch (error) {
      hostLog("Cannot serialize response:", error);
      resolve();
      return;
    }

    const timer = setTimeout(resolve, STDOUT_FLUSH_GRACE_MS);
    try {
      process.stdout.write(line, () => {
        clearTimeout(timer);
        resolve();
      });
    } catch (error) {
      clearTimeout(timer);
      hostLog("Cannot write response:", error);
      resolve();
    }
  });
}

function flushStdout() {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, STDOUT_FLUSH_GRACE_MS);
    try {
      process.stdout.write("", () => {
        clearTimeout(timer);
        resolve();
      });
    } catch {
      clearTimeout(timer);
      resolve();
    }
  });
}

function normalizeCommands(pluginId, commands) {
  if (commands === undefined) return [];
  if (!Array.isArray(commands)) {
    throw pluginError("manifest", `Plugin "${pluginId}" commands must be an array`);
  }

  const ids = [];
  for (const item of commands) {
    if (typeof item === "string" && item.length > 0) {
      ids.push(item);
    } else if (item && typeof item === "object" && typeof item.command === "string" && item.command.length > 0) {
      ids.push(item.command);
    } else {
      throw pluginError("manifest", `Plugin "${pluginId}" has an invalid command entry`);
    }
  }
  return [...new Set(ids)];
}

function normalizeTools(pluginId, tools) {
  if (tools === undefined) return [];
  if (!Array.isArray(tools)) {
    throw pluginError("manifest", `Plugin "${pluginId}" tools must be an array`);
  }

  const normalized = [];
  for (const item of tools) {
    if (typeof item === "string" && item.length > 0) {
      normalized.push({ name: item, command: item });
    } else if (
      item &&
      typeof item === "object" &&
      typeof item.name === "string" &&
      item.name.length > 0
    ) {
      normalized.push({
        name: item.name,
        command:
          typeof item.command === "string" && item.command.length > 0 ? item.command : item.name
      });
    } else {
      throw pluginError("manifest", `Plugin "${pluginId}" has an invalid tool entry`);
    }
  }
  return normalized;
}

function describePlugin(spec) {
  if (!spec || typeof spec !== "object" || Array.isArray(spec)) {
    throw pluginError("manifest", "Plugin entry must be an object");
  }
  if (typeof spec.id !== "string" || spec.id.length === 0) {
    throw pluginError("manifest", "Plugin id must be a non-empty string");
  }
  if (typeof spec.path !== "string" || spec.path.length === 0) {
    throw pluginError("manifest", `Plugin "${spec.id}" path must be a non-empty string`);
  }
  if (typeof spec.main !== "string" || spec.main.length === 0) {
    throw pluginError("manifest", `Plugin "${spec.id}" main must be a non-empty string`);
  }

  const pluginPath = path.resolve(spec.path);
  const entry = path.resolve(pluginPath, spec.main);
  if (entry === pluginPath || !isPathInside(pluginPath, entry)) {
    throw pluginError("manifest", `Plugin "${spec.id}" main must stay inside its plugin directory`);
  }

  return {
    id: spec.id,
    path: pluginPath,
    entry,
    commands: normalizeCommands(spec.id, spec.commands),
    tools: normalizeTools(spec.id, spec.tools)
  };
}

async function loadPlugin(descriptor, pluginWorkspace) {
  let module;
  try {
    module = await import(pathToFileURL(descriptor.entry).href);
  } catch (error) {
    throw pluginError("import", `Failed to import plugin "${descriptor.id}" from ${descriptor.entry}: ${messageOf(error)}`);
  }

  const defaultExport = module.default && typeof module.default === "object" ? module.default : null;
  const activate = module.activate ?? defaultExport?.activate;
  const deactivate = module.deactivate ?? defaultExport?.deactivate;
  if (typeof activate !== "function") {
    throw pluginError("activate", `Plugin "${descriptor.id}" must export an activate(ctx) function`);
  }

  let register;
  let registerTool;
  try {
    register = commandRegistry.createPlugin(descriptor.id, descriptor.commands);
    registerTool = toolRegistry.createPlugin(descriptor.id, descriptor.tools);
  } catch (error) {
    throw pluginError("manifest", `Cannot register plugin "${descriptor.id}": ${messageOf(error)}`);
  }

  try {
    const storage = createPluginStorage(descriptor.id, descriptor.path, (...args) => pluginLog(descriptor.id, ...args));
    const ctx = Object.freeze({
      commands: Object.freeze({ register }),
      tools: Object.freeze({ register: registerTool }),
      workspace: pluginWorkspace,
      storage,
      window: Object.freeze({
        showMessage(text) {
          pluginLog(descriptor.id, `showMessage: ${String(text)}`);
        }
      }),
      log: (...args) => pluginLog(descriptor.id, ...args)
    });

    await activate(ctx);
  } catch (error) {
    commandRegistry.remove(descriptor.id);
    toolRegistry.remove(descriptor.id);
    throw pluginError("activate", `Plugin "${descriptor.id}" failed to activate: ${messageOf(error)}`);
  }

  return {
    id: descriptor.id,
    path: descriptor.path,
    entry: descriptor.entry,
    deactivate: typeof deactivate === "function" ? deactivate : null
  };
}

async function handleInitialize(id, params) {
  if (initialized) {
    return errorResponse(id, RPC_INTERNAL_ERROR, "Extension host is already initialized");
  }
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    return errorResponse(id, RPC_INVALID_PARAMS, "initialize expects an object with workspaceRoot and plugins");
  }

  let nextWorkspace;
  try {
    nextWorkspace = createWorkspace(params.workspaceRoot);
  } catch (error) {
    return errorResponse(id, RPC_INVALID_PARAMS, `Invalid workspaceRoot: ${messageOf(error)}`);
  }
  if (!Array.isArray(params.plugins)) {
    return errorResponse(id, RPC_INVALID_PARAMS, "initialize.plugins must be an array");
  }

  const errors = [];
  const plugins = [];
  const seen = new Set();

  for (const spec of params.plugins) {
    let pluginId = spec && typeof spec.id === "string" ? spec.id : null;
    try {
      const descriptor = describePlugin(spec);
      pluginId = descriptor.id;
      if (seen.has(descriptor.id)) {
        throw pluginError("manifest", `Duplicate plugin id "${descriptor.id}"`);
      }
      seen.add(descriptor.id);
      plugins.push(await loadPlugin(descriptor, nextWorkspace));
    } catch (error) {
      const details = thrownDetails(error);
      errors.push({
        pluginId,
        stage: error && typeof error.pluginStage === "string" ? error.pluginStage : "load",
        message: details.message
      });
    }
  }

  loadedPlugins = plugins;
  initialized = true;

  return resultResponse(id, {
    workspaceRoot: nextWorkspace.root,
    plugins: plugins.map((plugin) => ({
      id: plugin.id,
      commands: commandRegistry.list(plugin.id),
      tools: toolRegistry.list(plugin.id)
    })),
    errors
  });
}

async function handleExecute(id, params) {
  if (!initialized) {
    return errorResponse(id, RPC_INTERNAL_ERROR, "Extension host is not initialized");
  }
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    return errorResponse(id, RPC_INVALID_PARAMS, "command/execute expects an object with pluginId and command");
  }

  const { pluginId, command } = params;
  if (typeof pluginId !== "string" || pluginId.length === 0) {
    return errorResponse(id, RPC_INVALID_PARAMS, "command/execute.pluginId must be a non-empty string");
  }
  if (typeof command !== "string" || command.length === 0) {
    return errorResponse(id, RPC_INVALID_PARAMS, "command/execute.command must be a non-empty string");
  }

  const found = commandRegistry.lookup(pluginId, command);
  if (!found.found) {
    return errorResponse(id, RPC_METHOD_NOT_FOUND, `Command not found: ${pluginId}/${command}`, {
      pluginId,
      command,
      reason: found.reason
    });
  }

  const handler = found.handler;
  try {
    const value = await handler(params.args);
    return resultResponse(id, serializableResult(value));
  } catch (error) {
    const details = thrownDetails(error);
    return errorResponse(id, RPC_INTERNAL_ERROR, `Command failed: ${pluginId}/${command}: ${details.message}`, {
      pluginId,
      command,
      name: details.name,
      stack: details.stack
    });
  }
}

async function handleToolExecute(id, params) {
  if (!initialized) {
    return errorResponse(id, RPC_INTERNAL_ERROR, "Extension host is not initialized");
  }
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    return errorResponse(id, RPC_INVALID_PARAMS, "tool/execute expects an object with pluginId and tool");
  }

  const { pluginId, tool } = params;
  if (typeof pluginId !== "string" || pluginId.length === 0) {
    return errorResponse(id, RPC_INVALID_PARAMS, "tool/execute.pluginId must be a non-empty string");
  }
  if (typeof tool !== "string" || tool.length === 0) {
    return errorResponse(id, RPC_INVALID_PARAMS, "tool/execute.tool must be a non-empty string");
  }

  const found = toolRegistry.lookup(pluginId, tool);
  if (!found.found) {
    return errorResponse(id, RPC_METHOD_NOT_FOUND, `Tool not found: ${pluginId}/${tool}`, {
      pluginId,
      tool,
      reason: found.reason
    });
  }

  // ctx.tools.register 优先；未注册回退到清单 command 映射的命令 handler。
  let handler = found.handler;
  if (!handler) {
    const command = commandRegistry.lookup(pluginId, found.command);
    if (!command.found) {
      return errorResponse(
        id,
        RPC_METHOD_NOT_FOUND,
        `Tool handler missing: ${pluginId}/${tool} (command "${found.command}" not registered)`,
        { pluginId, tool, command: found.command, reason: command.reason }
      );
    }
    handler = command.handler;
  }

  try {
    const value = await handler(params.args);
    return resultResponse(id, { result: serializableResult(value) });
  } catch (error) {
    const details = thrownDetails(error);
    return errorResponse(id, RPC_INTERNAL_ERROR, `Tool failed: ${pluginId}/${tool}: ${details.message}`, {
      pluginId,
      tool,
      name: details.name,
      stack: details.stack
    });
  }
}

function dispatch(message, id) {
  switch (message.method) {
    case "initialize":
      return handleInitialize(id, message.params);
    case "command/execute":
      return handleExecute(id, message.params);
    case "tool/execute":
      return handleToolExecute(id, message.params);
    case "shutdown":
      return resultResponse(id, { ok: true });
    default:
      return errorResponse(id, RPC_METHOD_NOT_FOUND, `Method not found: ${message.method}`, {
        method: message.method
      });
  }
}

async function handleLine(rawLine) {
  const line = rawLine.replace(/^\uFEFF/, "").replace(/\r$/, "");
  if (line.trim() === "") return;

  let message;
  try {
    message = JSON.parse(line);
  } catch (error) {
    await writeResponse(errorResponse(null, RPC_PARSE_ERROR, `Parse error: ${messageOf(error)}`));
    return;
  }

  if (!message || typeof message !== "object" || Array.isArray(message)) {
    await writeResponse(errorResponse(null, RPC_INVALID_REQUEST, "Invalid Request: expected a JSON-RPC object"));
    return;
  }

  const hasId = Object.hasOwn(message, "id");
  const id = hasId ? message.id : null;

  if (message.jsonrpc !== "2.0") {
    if (hasId) {
      await writeResponse(errorResponse(id, RPC_INVALID_REQUEST, 'Invalid Request: "jsonrpc" must be "2.0"'));
    }
    return;
  }
  if (typeof message.method !== "string" || message.method.length === 0) {
    if (hasId) {
      await writeResponse(errorResponse(id, RPC_INVALID_REQUEST, "Invalid Request: method must be a non-empty string"));
    }
    return;
  }

  let response;
  try {
    response = await dispatch(message, id);
  } catch (error) {
    const details = thrownDetails(error);
    response = errorResponse(id, RPC_INTERNAL_ERROR, `Internal error: ${details.message}`, {
      name: details.name,
      stack: details.stack
    });
  }

  if (!hasId) {
    if (message.method === "shutdown") triggerShutdown(null);
    return;
  }

  if (message.method === "shutdown") {
    triggerShutdown(() => writeResponse(response));
    return;
  }

  writeResponse(response);
}

async function deactivatePlugins() {
  if (deactivated) return;
  deactivated = true;

  const deadline = Date.now() + DEACTIVATE_BUDGET_MS;
  const plugins = loadedPlugins.slice().reverse();
  loadedPlugins = [];
  initialized = false;

  for (const plugin of plugins) {
    if (typeof plugin.deactivate !== "function") continue;
    const remaining = deadline - Date.now();
    if (remaining <= 0) {
      hostLog(`Skipping deactivate for plugin "${plugin.id}": shutdown budget exhausted`);
      continue;
    }

    let timer;
    try {
      await Promise.race([
        Promise.resolve().then(() => plugin.deactivate()),
        new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error(`deactivate timed out after ${remaining}ms`)), remaining);
        })
      ]);
    } catch (error) {
      pluginLog(plugin.id, `deactivate failed: ${messageOf(error)}`);
    } finally {
      if (timer) clearTimeout(timer);
    }
  }
}

function triggerShutdown(respond) {
  if (shutdownPromise) return shutdownPromise;
  shuttingDown = true;
  shutdownPromise = (async () => {
    try {
      await deactivatePlugins();
      if (respond) {
        await Promise.race([Promise.resolve(respond()), delay(STDOUT_FLUSH_GRACE_MS)]);
      }
    } catch (error) {
      hostLog("Shutdown error:", error);
    } finally {
      process.exit(0);
    }
  })();
  return shutdownPromise;
}

async function run() {
  reserveStdout();
  process.stdin.setEncoding("utf8");

  process.on("unhandledRejection", (reason) => {
    hostLog("Unhandled rejection:", reason);
  });
  process.on("uncaughtException", (error) => {
    hostLog("Uncaught exception:", error);
  });
  process.on("SIGINT", () => {
    triggerShutdown(null);
  });
  process.on("SIGTERM", () => {
    triggerShutdown(null);
  });
  process.stdout.on("error", (error) => {
    if (error && error.code === "EPIPE") process.exit(0);
    hostLog("stdout error:", error);
  });

  let buffer = "";
  try {
    for await (const chunk of process.stdin) {
      buffer += chunk;
      let newlineIndex;
      while ((newlineIndex = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, newlineIndex);
        buffer = buffer.slice(newlineIndex + 1);
        if (!shuttingDown) await handleLine(line);
      }
      if (shuttingDown) break;
    }
    if (!shuttingDown && buffer.trim() !== "") {
      await handleLine(buffer);
    }
  } catch (error) {
    hostLog("stdin processing failed:", error);
  }

  if (!shuttingDown) {
    shuttingDown = true;
    try {
      await deactivatePlugins();
    } catch (error) {
      hostLog("Shutdown error:", error);
    }
    await flushStdout();
    process.exit(0);
  }
}

run().catch((error) => {
  hostLog("Fatal error:", error);
  process.exit(1);
});
