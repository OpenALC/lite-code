// 插件 UI 视图数据注册表（docs/plans/plugin-webview.md §3.2）：插件 activate 时经
// ctx.ui.register(viewId, provider) 注册 provider；ui/values 请求按
// pluginId + viewId 查找并调用 provider.getValues(sectionId)。
// 结构与 views.mjs 同构；sectionKinds 来自 initialize 载荷的节 kind 白名单
// （markdown 节无回读，宿主在派发前直接拒绝，不调用 provider）。
export function createUiRegistry() {
  const plugins = new Map(); // pluginId -> Map(viewId -> { provider, sectionKinds })

  function createPlugin(pluginId, views) {
    if (plugins.has(pluginId)) {
      throw new Error(`Plugin "${pluginId}" is already registered`);
    }
    const declared = new Map();
    for (const view of views) {
      if (!view || typeof view !== "object" || Array.isArray(view)) {
        throw new Error(`Plugin "${pluginId}" has an invalid ui view entry`);
      }
      if (typeof view.id !== "string" || view.id.length === 0) {
        throw new Error(`Plugin "${pluginId}" has an invalid ui view entry`);
      }
      const sectionKinds = Array.isArray(view.sectionKinds) ? view.sectionKinds : [];
      declared.set(view.id, { provider: null, sectionKinds });
    }
    plugins.set(pluginId, declared);

    return function register(viewId, provider) {
      if (typeof viewId !== "string" || viewId.length === 0) {
        throw new TypeError("ui view must be a non-empty string");
      }
      if (!provider || typeof provider.getValues !== "function") {
        throw new TypeError(`provider for ui view "${viewId}" must expose getValues(sectionId)`);
      }
      const views = plugins.get(pluginId);
      if (!views) {
        throw new Error(`Plugin "${pluginId}" is not registered`);
      }
      if (!views.has(viewId)) {
        throw new Error(`UI view "${viewId}" is not declared in the manifest of plugin "${pluginId}"`);
      }
      const entry = views.get(viewId);
      if (entry.provider) {
        throw new Error(`UI view "${viewId}" is already registered by plugin "${pluginId}"`);
      }
      entry.provider = provider;
    };
  }

  function remove(pluginId) {
    plugins.delete(pluginId);
  }

  // 命中返回 { found: true, provider, sectionKinds }；未命中返回 { found: false,
  // reason }：plugin-not-loaded（插件不在注册表）/ view-not-declared（清单未声明）/
  // view-not-registered（activate 未注册 provider）。
  function lookup(pluginId, viewId) {
    const views = plugins.get(pluginId);
    if (!views) return { found: false, reason: "plugin-not-loaded" };
    if (!views.has(viewId)) return { found: false, reason: "view-not-declared" };
    const entry = views.get(viewId);
    if (!entry.provider) return { found: false, reason: "view-not-registered" };
    return { found: true, provider: entry.provider, sectionKinds: entry.sectionKinds };
  }

  function list(pluginId) {
    const views = plugins.get(pluginId);
    return views ? [...views.keys()] : [];
  }

  return { createPlugin, remove, lookup, list };
}
