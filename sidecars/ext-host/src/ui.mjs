// 插件 UI 视图数据注册表（docs/plans/plugin-webview.md §3.2）：插件 activate 时经
// ctx.ui.register(viewId, provider) 注册 provider；ui/values 请求按
// (pluginId, viewId) 定位视图，再以稳定节 id 查节描述（initialize 载荷携带的
// 完整 DTO，Rust 校验期已保证形状，宿主再兜一层），命中后调用
// provider.getValues(sectionId)。注册表按 (pluginId, viewId) 存
// 「节 id → { provider, description }」槽位；无 source:"provider" 的节
// （markdown/form/静态 kv）由 host.mjs 在调用 provider 前拒绝。
export function createUiRegistry() {
  const plugins = new Map(); // pluginId -> Map(viewId -> { provider, sections })

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
      if (declared.has(view.id)) {
        throw new Error(`Plugin "${pluginId}" has a duplicate ui view "${view.id}"`);
      }
      const sections = new Map(); // sectionId -> description（createPlugin 按描述建槽位）
      for (const section of view.sections ?? []) {
        if (!section || typeof section !== "object" || Array.isArray(section)) {
          throw new Error(`Plugin "${pluginId}" has an invalid ui section entry`);
        }
        if (typeof section.id !== "string" || section.id.length === 0) {
          throw new Error(`Plugin "${pluginId}" has an invalid ui section entry`);
        }
        if (sections.has(section.id)) {
          throw new Error(`UI view "${view.id}" of plugin "${pluginId}" has a duplicate section "${section.id}"`);
        }
        sections.set(section.id, section);
      }
      declared.set(view.id, { provider: null, sections });
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

  // 命中返回 { found: true, provider, sections }（sections 为
  // 节 id -> 描述 的 Map，provider 与描述由调用方按节 id 组合派发）；
  // 未命中返回 { found: false, reason }：plugin-not-loaded（插件不在注册表）/
  // view-not-declared（清单未声明）/ view-not-registered（activate 未注册 provider）。
  function lookup(pluginId, viewId) {
    const views = plugins.get(pluginId);
    if (!views) return { found: false, reason: "plugin-not-loaded" };
    if (!views.has(viewId)) return { found: false, reason: "view-not-declared" };
    const entry = views.get(viewId);
    if (!entry.provider) return { found: false, reason: "view-not-registered" };
    return { found: true, provider: entry.provider, sections: entry.sections };
  }

  // initialize 结果回显完整节描述 DTO（docs/plans/plugin-webview.md §3.2）：
  // [{ id, sections: [{ id, kind, ... }] }]。
  function list(pluginId) {
    const views = plugins.get(pluginId);
    if (!views) return [];
    return [...views.entries()].map(([id, entry]) => ({
      id,
      sections: [...entry.sections.values()]
    }));
  }

  return { createPlugin, remove, lookup, list };
}
