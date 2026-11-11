import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { HostClient, createFixture, initializeHost, writePlugin } from "./helpers.mjs";

// M2-R（docs/plans/plugin-webview.md §3.2/§5）：字节规模上限、跨插件同名视图、
// 旧 sectionKinds 载荷拒绝、重复声明拒绝，以及 ui/values 全程不触碰命令 handler。

test("ui/values enforces string and total byte limits", async (t) => {
  const fixture = await createFixture(t);
  const limits = await writePlugin(fixture.pluginsRoot, "test.limits", {
    commands: [],
    uiViews: [
      {
        id: "test.limits.board",
        sections: [
          { id: "big-cell", kind: "list", source: "provider", columns: [{ key: "title" }] },
          { id: "edge-cell", kind: "list", source: "provider", columns: [{ key: "title" }] },
          { id: "big-total", kind: "list", source: "provider", columns: [{ key: "title" }, { key: "state" }] }
        ]
      }
    ],
    source: `
      export function activate(ctx) {
        ctx.ui.register("test.limits.board", {
          getValues(sectionId) {
            if (sectionId === "big-cell") {
              // 6000 个 CJK 字符 = 18000 UTF-8 字节 > 16 KiB。
              return { rows: [{ title: "\\u6c49".repeat(6000) }] };
            }
            if (sectionId === "edge-cell") {
              return { rows: [{ title: "z".repeat(16384) }] };
            }
            if (sectionId === "big-total") {
              // 单行字符串均合法，但 100×3000 字节 > 256 KiB 总量。
              return {
                rows: Array(100).fill(null).map(() => ({ title: "y".repeat(3000), state: "ok" }))
              };
            }
            return null;
          }
        });
      }
      export function deactivate() {}
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      { ...limits, main: path.join(limits.path, "index.mjs") }
    ]);
    assert.deepEqual(result.errors, []);

    // 单字符串 UTF-8 超过 16 KiB：invalid-values。
    const bigCell = await client.uiValues("test.limits", "test.limits.board", "big-cell", "bl-1");
    assert.equal(bigCell.error.code, -32000);
    assert.equal(bigCell.error.data.reason, "invalid-values");
    assert.match(bigCell.error.message, /16384 bytes/);

    // 恰好 16 KiB 的字符串合法。
    const edgeCell = await client.uiValues("test.limits", "test.limits.board", "edge-cell", "bl-2");
    assert.equal(edgeCell.error, undefined);
    assert.equal(edgeCell.result.rows[0].title.length, 16384);

    // 归一后序列化总量超过 256 KiB：invalid-values。
    const bigTotal = await client.uiValues("test.limits", "test.limits.board", "big-total", "bl-3");
    assert.equal(bigTotal.error.code, -32000);
    assert.equal(bigTotal.error.data.reason, "invalid-values");
    assert.match(bigTotal.error.message, /262144 bytes/);

    // kv 键同样受单字符串上限约束。
    const kvFixture = await createFixture(t);
    const kvLimits = await writePlugin(kvFixture.pluginsRoot, "test.kv-limits", {
      commands: [],
      uiViews: [
        {
          id: "test.kv-limits.board",
          sections: [{ id: "items", kind: "kv", source: "provider" }]
        }
      ],
      source: `
        export function activate(ctx) {
          ctx.ui.register("test.kv-limits.board", {
            getValues() {
              return { items: [{ key: "k".repeat(16385), value: "v" }] };
            }
          });
        }
      `
    });
    const kvClient = new HostClient(kvFixture.workspaceRoot);
    try {
      await initializeHost(kvClient, kvFixture, [
        kvFixture.spec,
        { ...kvLimits, main: path.join(kvLimits.path, "index.mjs") }
      ]);
      const bigKey = await kvClient.uiValues("test.kv-limits", "test.kv-limits.board", "items", "bl-4");
      assert.equal(bigKey.error.code, -32000);
      assert.equal(bigKey.error.data.reason, "invalid-values");
      assert.match(bigKey.error.message, /16384 bytes/);
      assert.deepEqual(kvClient.protocolErrors, []);
    } finally {
      await kvClient.close();
    }

    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("same viewId in different plugins stays isolated and never invokes command handlers", async (t) => {
  const fixture = await createFixture(t);
  const sections = (titleKey) => [
    { id: "tasks", kind: "list", source: "provider", columns: [{ key: titleKey }] }
  ];
  const alpha = await writePlugin(fixture.pluginsRoot, "test.alpha", {
    commands: ["test.alpha.run"],
    uiViews: [{ id: "board", sections: sections("alpha-title") }],
    source: `
      export function activate(ctx) {
        ctx.commands.register("test.alpha.run", () => {
          ctx.log("alpha command handler called");
          return "alpha";
        });
        ctx.ui.register("board", {
          getValues(sectionId) {
            ctx.log("alpha getValues " + sectionId);
            return { rows: [{ "alpha-title": "from alpha" }] };
          }
        });
      }
    `
  });
  const beta = await writePlugin(fixture.pluginsRoot, "test.beta", {
    commands: ["test.beta.run"],
    uiViews: [{ id: "board", sections: sections("beta-title") }],
    source: `
      export function activate(ctx) {
        ctx.commands.register("test.beta.run", () => {
          ctx.log("beta command handler called");
          return "beta";
        });
        ctx.ui.register("board", {
          getValues(sectionId) {
            ctx.log("beta getValues " + sectionId);
            return { rows: [{ "beta-title": "from beta" }] };
          }
        });
      }
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      { ...alpha, main: path.join(alpha.path, "index.mjs") },
      { ...beta, main: path.join(beta.path, "index.mjs") }
    ]);
    assert.deepEqual(result.errors, []);
    assert.deepEqual(result.plugins.find((p) => p.id === "test.alpha").uiViews, [
      { id: "board", sections: [{ id: "tasks", kind: "list", source: "provider", columns: [{ key: "alpha-title" }] }] }
    ]);
    assert.deepEqual(result.plugins.find((p) => p.id === "test.beta").uiViews, [
      { id: "board", sections: [{ id: "tasks", kind: "list", source: "provider", columns: [{ key: "beta-title" }] }] }
    ]);

    // 同名视图按 (pluginId, viewId) 定位，各自 provider 收到稳定节 id。
    const alphaValues = await client.uiValues("test.alpha", "board", "tasks", "iso-1");
    assert.deepEqual(alphaValues.result, { rows: [{ "alpha-title": "from alpha" }] });
    const betaValues = await client.uiValues("test.beta", "board", "tasks", "iso-2");
    assert.deepEqual(betaValues.result, { rows: [{ "beta-title": "from beta" }] });
    assert.match(client.stderr, /alpha getValues tasks/);
    assert.match(client.stderr, /beta getValues tasks/);

    // ui/values 只调 provider，不触碰命令 handler（valuesCommand 已移除）。
    assert.doesNotMatch(client.stderr, /command handler called/);

    // 插件自己的 provider 只收到自己的调用。
    assert.equal((client.stderr.match(/alpha getValues/g) ?? []).length, 1);
    assert.equal((client.stderr.match(/beta getValues/g) ?? []).length, 1);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("legacy sectionKinds payloads and duplicate declarations are rejected at initialize", async (t) => {
  const fixture = await createFixture(t);
  const legacy = await writePlugin(fixture.pluginsRoot, "test.legacy-payload", {
    commands: [],
    uiViews: [{ id: "board", sectionKinds: ["list"] }],
    source: `
      export function activate(ctx) {}
    `
  });
  const duplicateSection = await writePlugin(fixture.pluginsRoot, "test.dup-section", {
    commands: [],
    uiViews: [
      {
        id: "board",
        sections: [
          { id: "tasks", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "tasks", kind: "kv", source: "provider" }
        ]
      }
    ],
    source: `
      export function activate(ctx) {}
    `
  });
  const duplicateView = await writePlugin(fixture.pluginsRoot, "test.dup-view", {
    commands: [],
    uiViews: [
      { id: "board", sections: [{ id: "a", kind: "markdown" }] },
      { id: "board", sections: [{ id: "b", kind: "markdown" }] }
    ],
    source: `
      export function activate(ctx) {}
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      { ...legacy, main: path.join(legacy.path, "index.mjs") },
      { ...duplicateSection, main: path.join(duplicateSection.path, "index.mjs") },
      { ...duplicateView, main: path.join(duplicateView.path, "index.mjs") }
    ]);
    // 三个非法插件各自只标 error，不影响示例插件。
    assert.equal(result.errors.length, 3);
    const errors = new Map(result.errors.map((error) => [error.pluginId, error]));
    assert.equal(errors.get("test.legacy-payload").stage, "manifest");
    assert.match(errors.get("test.legacy-payload").message, /legacy sectionKinds payload/);
    assert.equal(errors.get("test.dup-section").stage, "manifest");
    assert.match(errors.get("test.dup-section").message, /duplicate section "tasks"/);
    assert.equal(errors.get("test.dup-view").stage, "manifest");
    assert.match(errors.get("test.dup-view").message, /duplicate ui view "board"/);
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello"]
    );

    // 被拒绝的插件不在注册表：-32601 plugin-not-loaded。
    const missing = await client.uiValues("test.legacy-payload", "board", "tasks", "lg-1");
    assert.equal(missing.error.code, -32601);
    assert.equal(missing.error.data.reason, "plugin-not-loaded");
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});
