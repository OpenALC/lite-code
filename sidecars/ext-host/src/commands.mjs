export function createCommandRegistry() {
  const plugins = new Map();
  const owners = new Map();

  function createPlugin(pluginId, commandIds) {
    if (plugins.has(pluginId)) {
      throw new Error(`Plugin "${pluginId}" is already registered`);
    }
    const allowed = new Set(commandIds);
    plugins.set(pluginId, { allowed, handlers: new Map() });

    return function register(command, handler) {
      if (typeof command !== "string" || command.length === 0) {
        throw new TypeError("command must be a non-empty string");
      }
      if (typeof handler !== "function") {
        throw new TypeError(`handler for command "${command}" must be a function`);
      }
      if (!allowed.has(command)) {
        throw new Error(`Command "${command}" is not declared in the manifest of plugin "${pluginId}"`);
      }
      const owner = owners.get(command);
      if (owner && owner !== pluginId) {
        throw new Error(`Command "${command}" is already registered by plugin "${owner}"`);
      }
      if (plugins.get(pluginId).handlers.has(command)) {
        throw new Error(`Command "${command}" is already registered by plugin "${pluginId}"`);
      }
      plugins.get(pluginId).handlers.set(command, handler);
      owners.set(command, pluginId);
    };
  }

  function remove(pluginId) {
    const entry = plugins.get(pluginId);
    if (!entry) return;
    for (const command of entry.handlers.keys()) {
      if (owners.get(command) === pluginId) owners.delete(command);
    }
    plugins.delete(pluginId);
  }

  function lookup(pluginId, command) {
    const entry = plugins.get(pluginId);
    if (!entry) return { found: false, reason: "plugin-not-loaded" };
    const handler = entry.handlers.get(command);
    if (!handler) return { found: false, reason: "command-not-registered" };
    return { found: true, handler };
  }

  function list(pluginId) {
    const entry = plugins.get(pluginId);
    return entry ? [...entry.allowed] : [];
  }

  return { createPlugin, remove, lookup, list };
}
