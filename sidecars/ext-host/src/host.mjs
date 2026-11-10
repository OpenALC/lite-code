import path from "node:path";
import { pathToFileURL } from "node:url";

import { createCommandRegistry } from "./commands.mjs";
import { createPluginStorage } from "./storage.mjs";
import { createToolRegistry } from "./tools.mjs";
import { createViewRegistry } from "./views.mjs";
import { createWorkspace, isPathInside } from "./workspace.mjs";
import { RPC_INVALID_PARAMS, RPC_INVALID_REQUEST, RPC_INTERNAL_ERROR, RPC_METHOD_NOT_FOUND, RPC_PARSE_ERROR, errorResponse, resultResponse, serializableResult, thrownDetails } from "./jsonrpc.mjs";

const DEACTIVATE_BUDGET_MS = 1500;
const STDOUT_FLUSH_GRACE_MS = 500;
// view/children provider 超时（docs/EXTENSIONS.md §4，与 tool/execute 同为
// 30s，Rust 侧另有 recv_timeout 兜底）；测试可经 LC_EXT_HOST_VIEW_TIMEOUT_MS
// 注入短超时。
const VIEW_TIMEOUT_MS = (() => {
  const raw = Number(process.env.LC_EXT_HOST_VIEW_TIMEOUT_MS);
  return Number.isFinite(raw) && raw > 0 ? Math.floor(raw) : 30_000;
})();

const stderrLog = console.error.bind(console);
const commandRegistry = createCommandRegistry();
const toolRegistry = createToolRegistry();
const viewRegistry = createViewRegistry();

let loadedPlugins = [];
let pluginWorkspace = null;
const pendingPlugins = new Map(); // pluginId -> { descriptor, state, promise, stage, message }
const pendingCommands = new Map(); // command -> pluginId（仅延迟插件声明的命令）
const pendingTools = new Map(); // tool name -> pluginId（仅延迟插件声明的工具）
const pendingViews = new Map(); // viewId -> pluginId（仅延迟插件声明的视图）
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

function normalizeActivationEvents(pluginId, events) {
  if (events === undefined || events === null) return [];
  if (!Array.isArray(events)) {
    throw pluginError("manifest", `Plugin "${pluginId}" activationEvents must be an array`);
  }

  const normalized = [];
  for (const event of events) {
    if (typeof event !== "string" || event.trim().length === 0) {
      throw pluginError("manifest", `Plugin "${pluginId}" has an invalid activation event`);
    }
    const trimmed = event.trim();
    if (!normalized.includes(trimmed)) normalized.push(trimmed);
  }
  return normalized;
}

// initialize 载荷里的视图声明：接受纯 id 字符串或 { id, name } 对象，
// 宿主只按 id 登记与查找（docs/EXTENSIONS.md §4）。
function normalizeViews(pluginId, views) {
  if (views === undefined || views === null) return [];
  if (!Array.isArray(views)) {
    throw pluginError("manifest", `Plugin "${pluginId}" views must be an array`);
  }

  const ids = [];
  for (const item of views) {
    if (typeof item === "string" && item.length > 0) {
      ids.push(item);
    } else if (item && typeof item === "object" && typeof item.id === "string" && item.id.length > 0) {
      ids.push(item.id);
    } else {
      throw pluginError("manifest", `Plugin "${pluginId}" has an invalid view entry`);
    }
  }
  return [...new Set(ids)];
}

// 视图节点形状校验（docs/EXTENSIONS.md §4）：id/label 必填非空字符串，
// icon/command/tooltip 可空字符串；其余键一律丢弃。形状非法抛错。
function normalizeViewNodes(pluginId, viewId, raw) {
  if (raw === undefined || raw === null) return [];
  if (!Array.isArray(raw)) {
    throw pluginError("manifest", `View "${viewId}" of plugin "${pluginId}" must return an array of nodes`);
  }

  return raw.map((item, index) => {
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw pluginError("manifest", `View "${viewId}" of plugin "${pluginId}" returned an invalid node at index ${index}`);
    }
    if (typeof item.id !== "string" || item.id.length === 0) {
      throw pluginError("manifest", `View "${viewId}" of plugin "${pluginId}" returned a node without a non-empty string id at index ${index}`);
    }
    if (typeof item.label !== "string" || item.label.length === 0) {
      throw pluginError("manifest", `View "${viewId}" of plugin "${pluginId}" returned node "${item.id}" without a non-empty string label`);
    }
    const node = { id: item.id, label: item.label };
    if (typeof item.icon === "string" && item.icon.length > 0) node.icon = item.icon;
    if (typeof item.command === "string" && item.command.length > 0) node.command = item.command;
    if (typeof item.tooltip === "string" && item.tooltip.length > 0) node.tooltip = item.tooltip;
    return node;
  });
}

// 延迟判定是插件级的：activationEvents 非空且不含 `*` 即为延迟插件，
// 任一声明的 command/tool/view 被调用时激活整个插件（docs/EXTENSIONS.md §4）。
function isLazyDescriptor(descriptor) {
  return descriptor.activationEvents.length > 0 && !descriptor.activationEvents.includes("*");
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
    tools: normalizeTools(spec.id, spec.tools),
    views: normalizeViews(spec.id, spec.views),
    activationEvents: normalizeActivationEvents(spec.id, spec.activationEvents)
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
  let registerView;
  try {
    register = commandRegistry.createPlugin(descriptor.id, descriptor.commands);
    registerTool = toolRegistry.createPlugin(descriptor.id, descriptor.tools);
    registerView = viewRegistry.createPlugin(descriptor.id, descriptor.views);
  } catch (error) {
    throw pluginError("manifest", `Cannot register plugin "${descriptor.id}": ${messageOf(error)}`);
  }

  try {
    const storage = createPluginStorage(descriptor.id, descriptor.path, (...args) => pluginLog(descriptor.id, ...args));
    const ctx = Object.freeze({
      commands: Object.freeze({ register }),
      tools: Object.freeze({ register: registerTool }),
      views: Object.freeze({ register: registerView }),
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
    viewRegistry.remove(descriptor.id);
    throw pluginError("activate", `Plugin "${descriptor.id}" failed to activate: ${messageOf(error)}`);
  }

  return {
    id: descriptor.id,
    path: descriptor.path,
    entry: descriptor.entry,
    deactivate: typeof deactivate === "function" ? deactivate : null
  };
}

// 延迟插件不 import/activate，只登记 command/tool/view -> pluginId 映射，
// 首次命中时由 activatePendingPlugin 懒激活（docs/EXTENSIONS.md §4）。
function registerPendingPlugin(descriptor) {
  if (pendingPlugins.has(descriptor.id)) {
    throw pluginError("manifest", `Duplicate plugin id "${descriptor.id}"`);
  }
  for (const command of descriptor.commands) {
    const owner = pendingCommands.get(command);
    if (owner && owner !== descriptor.id) {
      throw pluginError("manifest", `Command "${command}" is already declared by pending plugin "${owner}"`);
    }
  }
  for (const tool of descriptor.tools) {
    const owner = pendingTools.get(tool.name);
    if (owner && owner !== descriptor.id) {
      throw pluginError("manifest", `Tool "${tool.name}" is already declared by pending plugin "${owner}"`);
    }
  }
  for (const view of descriptor.views) {
    const owner = pendingViews.get(view);
    if (owner && owner !== descriptor.id) {
      throw pluginError("manifest", `View "${view}" is already declared by pending plugin "${owner}"`);
    }
  }

  pendingPlugins.set(descriptor.id, {
    descriptor,
    state: "pending",
    promise: null,
    stage: null,
    message: null
  });
  for (const command of descriptor.commands) pendingCommands.set(command, descriptor.id);
  for (const tool of descriptor.tools) pendingTools.set(tool.name, descriptor.id);
  for (const view of descriptor.views) pendingViews.set(view, descriptor.id);
}

function forgetPendingPlugin(pluginId) {
  pendingPlugins.delete(pluginId);
  for (const [command, owner] of pendingCommands) {
    if (owner === pluginId) pendingCommands.delete(command);
  }
  for (const [tool, owner] of pendingTools) {
    if (owner === pluginId) pendingTools.delete(tool);
  }
  for (const [view, owner] of pendingViews) {
    if (owner === pluginId) pendingViews.delete(view);
  }
}

// 懒激活幂等：并发/重复调用共享同一次激活；失败状态记录后不重试。
// 返回 null 表示不是延迟插件；否则返回 { ok, stage?, message? }。
function activatePendingPlugin(pluginId) {
  const pending = pendingPlugins.get(pluginId);
  if (!pending) return null;
  if (pending.state === "failed") {
    return { ok: false, stage: pending.stage, message: pending.message };
  }
  if (!pending.promise) {
    pending.state = "activating";
    pending.promise = loadPlugin(pending.descriptor, pluginWorkspace)
      .then((plugin) => {
        forgetPendingPlugin(pluginId);
        loadedPlugins.push(plugin);
        return { ok: true };
      })
      .catch((error) => {
        pending.state = "failed";
        pending.stage = error && typeof error.pluginStage === "string" ? error.pluginStage : "activate";
        pending.message = messageOf(error);
        hostLog(`Lazy activation failed for plugin "${pluginId}" (stage: ${pending.stage}): ${pending.message}`);
        return { ok: false, stage: pending.stage, message: pending.message };
      });
  }
  return pending.promise;
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
      if (isLazyDescriptor(descriptor)) {
        registerPendingPlugin(descriptor);
      } else {
        plugins.push(await loadPlugin(descriptor, nextWorkspace));
      }
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
  pluginWorkspace = nextWorkspace;
  initialized = true;

  return resultResponse(id, {
    workspaceRoot: nextWorkspace.root,
    plugins: plugins.map((plugin) => ({
      id: plugin.id,
      commands: commandRegistry.list(plugin.id),
      tools: toolRegistry.list(plugin.id),
      views: viewRegistry.list(plugin.id)
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

  let found = commandRegistry.lookup(pluginId, command);
  if (!found.found && pendingCommands.get(command) === pluginId) {
    const activation = await activatePendingPlugin(pluginId);
    if (activation && !activation.ok) {
      return errorResponse(
        id,
        RPC_METHOD_NOT_FOUND,
        `Plugin "${pluginId}" failed to activate: ${activation.message}`,
        { pluginId, command, reason: "activation-failed", stage: activation.stage }
      );
    }
    found = commandRegistry.lookup(pluginId, command);
  }
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

  let found = toolRegistry.lookup(pluginId, tool);
  if (!found.found && pendingTools.get(tool) === pluginId) {
    const activation = await activatePendingPlugin(pluginId);
    if (activation && !activation.ok) {
      return errorResponse(
        id,
        RPC_METHOD_NOT_FOUND,
        `Plugin "${pluginId}" failed to activate: ${activation.message}`,
        { pluginId, tool, reason: "activation-failed", stage: activation.stage }
      );
    }
    found = toolRegistry.lookup(pluginId, tool);
  }
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

async function handleViewChildren(id, params) {
  if (!initialized) {
    return errorResponse(id, RPC_INTERNAL_ERROR, "Extension host is not initialized");
  }
  if (!params || typeof params !== "object" || Array.isArray(params)) {
    return errorResponse(id, RPC_INVALID_PARAMS, "view/children expects an object with pluginId and viewId");
  }

  const { pluginId, viewId } = params;
  if (typeof pluginId !== "string" || pluginId.length === 0) {
    return errorResponse(id, RPC_INVALID_PARAMS, "view/children.pluginId must be a non-empty string");
  }
  if (typeof viewId !== "string" || viewId.length === 0) {
    return errorResponse(id, RPC_INVALID_PARAMS, "view/children.viewId must be a non-empty string");
  }
  const nodeId =
    typeof params.nodeId === "string" && params.nodeId.length > 0 ? params.nodeId : undefined;

  // 延迟插件：命中其声明的视图（pendingViews 维度，docs/EXTENSIONS.md §4）
  // 时先懒激活再派发，语义与 onCommand 一致。
  let found = viewRegistry.lookup(pluginId, viewId);
  if (!found.found && pendingViews.get(viewId) === pluginId) {
    const activation = await activatePendingPlugin(pluginId);
    if (activation && !activation.ok) {
      return errorResponse(
        id,
        RPC_METHOD_NOT_FOUND,
        `Plugin "${pluginId}" failed to activate: ${activation.message}`,
        { pluginId, viewId, reason: "activation-failed", stage: activation.stage }
      );
    }
    found = viewRegistry.lookup(pluginId, viewId);
  }
  if (!found.found) {
    return errorResponse(id, RPC_METHOD_NOT_FOUND, `View not found: ${pluginId}/${viewId}`, {
      pluginId,
      viewId,
      reason: found.reason
    });
  }

  const provider = found.provider;
  let timer;
  try {
    // provider 可返回 Promise；超时（默认 30s，测试可注入短超时）返回
    // -32000 reason=provider-timeout。Rust 侧另有 recv_timeout 兜底。
    const raw = await Promise.race([
      Promise.resolve().then(() => provider.getChildren(nodeId)),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(pluginError("timeout", `View provider timed out after ${VIEW_TIMEOUT_MS}ms`)),
          VIEW_TIMEOUT_MS
        );
      })
    ]);
    const nodes = normalizeViewNodes(pluginId, viewId, raw);
    return resultResponse(id, nodes);
  } catch (error) {
    const details = thrownDetails(error);
    if (error && error.pluginStage === "timeout") {
      return errorResponse(id, RPC_INTERNAL_ERROR, details.message, {
        pluginId,
        viewId,
        reason: "provider-timeout"
      });
    }
    if (error && error.pluginStage === "manifest") {
      // 节点形状非法：id/label 必填，属插件缺陷，按内部错误上报。
      return errorResponse(id, RPC_INTERNAL_ERROR, details.message, {
        pluginId,
        viewId,
        reason: "invalid-node"
      });
    }
    return errorResponse(id, RPC_INTERNAL_ERROR, `View failed: ${pluginId}/${viewId}: ${details.message}`, {
      pluginId,
      viewId,
      name: details.name,
      stack: details.stack
    });
  } finally {
    if (timer) clearTimeout(timer);
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
    case "view/children":
      return handleViewChildren(id, message.params);
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
  pendingPlugins.clear();
  pendingCommands.clear();
  pendingTools.clear();
  pendingViews.clear();
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
