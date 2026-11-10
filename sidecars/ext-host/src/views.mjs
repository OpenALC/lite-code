// 插件树视图注册表（docs/EXTENSIONS.md §4）：插件 activate 时经
// ctx.views.register(viewId, provider) 注册 provider；view/children 请求按
// pluginId + viewId 查找并调用 provider.getChildren(nodeId?)。
export function createViewRegistry() {
  const plugins = new Map(); // pluginId -> Map(viewId -> provider | null)

  function createPlugin(pluginId, viewIds) {
    if (plugins.has(pluginId)) {
      throw new Error(`Plugin "${pluginId}" is already registered`);
    }
    const declared = new Map();
    for (const viewId of viewIds) {
      if (typeof viewId !== "string" || viewId.length === 0) {
        throw new Error(`Plugin "${pluginId}" has an invalid view entry`);
      }
      declared.set(viewId, null); // provider 待 activate 时注册
    }
    plugins.set(pluginId, declared);

    return function register(viewId, provider) {
      if (typeof viewId !== "string" || viewId.length === 0) {
        throw new TypeError("view must be a non-empty string");
      }
      if (!provider || typeof provider.getChildren !== "function") {
        throw new TypeError(`provider for view "${viewId}" must expose getChildren(nodeId)`);
      }
      const views = plugins.get(pluginId);
      if (!views) {
        throw new Error(`Plugin "${pluginId}" is not registered`);
      }
      if (!views.has(viewId)) {
        throw new Error(`View "${viewId}" is not declared in the manifest of plugin "${pluginId}"`);
      }
      if (views.get(viewId)) {
        throw new Error(`View "${viewId}" is already registered by plugin "${pluginId}"`);
      }
      views.set(viewId, provider);
    };
  }

  function remove(pluginId) {
    plugins.delete(pluginId);
  }

  // 命中返回 { found: true, provider }；未命中返回 { found: false, reason }：
  // plugin-not-loaded（插件不在注册表）/ view-not-declared（清单未声明）/
  // view-not-registered（activate 未注册 provider）。
  function lookup(pluginId, viewId) {
    const views = plugins.get(pluginId);
    if (!views) return { found: false, reason: "plugin-not-loaded" };
    if (!views.has(viewId)) return { found: false, reason: "view-not-declared" };
    const provider = views.get(viewId);
    if (!provider) return { found: false, reason: "view-not-registered" };
    return { found: true, provider };
  }

  function list(pluginId) {
    const views = plugins.get(pluginId);
    return views ? [...views.keys()] : [];
  }

  return { createPlugin, remove, lookup, list };
}
