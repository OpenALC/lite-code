export function createToolRegistry() {
  const plugins = new Map();

  function createPlugin(pluginId, tools) {
    if (plugins.has(pluginId)) {
      throw new Error(`Plugin "${pluginId}" is already registered`);
    }
    const declared = new Map();
    for (const item of tools) {
      const name = typeof item === "string" ? item : item && typeof item === "object" ? item.name : null;
      if (typeof name !== "string" || name.length === 0) {
        throw new Error(`Plugin "${pluginId}" has an invalid tool entry`);
      }
      const command =
        item && typeof item === "object" && typeof item.command === "string" && item.command.length > 0
          ? item.command
          : name;
      declared.set(name, command);
    }
    plugins.set(pluginId, { declared, handlers: new Map() });

    return function register(tool, handler) {
      if (typeof tool !== "string" || tool.length === 0) {
        throw new TypeError("tool must be a non-empty string");
      }
      if (typeof handler !== "function") {
        throw new TypeError(`handler for tool "${tool}" must be a function`);
      }
      const entry = plugins.get(pluginId);
      if (!entry.declared.has(tool)) {
        throw new Error(`Tool "${tool}" is not declared in the manifest of plugin "${pluginId}"`);
      }
      if (entry.handlers.has(tool)) {
        throw new Error(`Tool "${tool}" is already registered by plugin "${pluginId}"`);
      }
      entry.handlers.set(tool, handler);
    };
  }

  function remove(pluginId) {
    plugins.delete(pluginId);
  }

  // 命中返回 { found: true, handler, command }；handler 可能为 null，
  // 表示插件未调用 ctx.tools.register，调用方回退到 command handler。
  function lookup(pluginId, tool) {
    const entry = plugins.get(pluginId);
    if (!entry) return { found: false, reason: "plugin-not-loaded" };
    const command = entry.declared.get(tool);
    if (command === undefined) return { found: false, reason: "tool-not-declared" };
    return { found: true, handler: entry.handlers.get(tool) ?? null, command };
  }

  function list(pluginId) {
    const entry = plugins.get(pluginId);
    return entry ? [...entry.declared.keys()] : [];
  }

  return { createPlugin, remove, lookup, list };
}
