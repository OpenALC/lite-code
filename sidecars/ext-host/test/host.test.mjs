import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { promisify } from "node:util";

import {
  BUILD_SCRIPT,
  HostClient,
  REPO_ROOT,
  createFixture,
  initializeHost,
  writePlugin
} from "./helpers.mjs";

const execFileAsync = promisify(execFile);

test("initialize loads the example plugin and executes a command", async (t) => {
  const fixture = await createFixture(t);
  const client = new HostClient(fixture.workspaceRoot);

  try {
    const result = await initializeHost(client, fixture);
    assert.deepEqual(result.errors, []);
    assert.equal(path.resolve(result.workspaceRoot), path.resolve(fixture.workspaceRoot));
    assert.deepEqual(result.plugins, [
      {
        id: "example.hello",
        commands: ["example.hello"],
        tools: ["example.hello.tool"],
        views: [],
        uiViews: []
      }
    ]);

    const first = await client.execute(
      "example.hello",
      "example.hello",
      { name: "Ada" },
      "req-1"
    );
    assert.equal(first.id, "req-1");
    assert.equal(first.error, undefined);
    assert.equal(first.result.message, "Hello, Ada!");
    assert.equal(first.result.readme, fixture.readme.slice(0, 80));
    assert.equal(first.result.calls, 1);
    assert.equal(path.resolve(first.result.workspace), path.resolve(fixture.workspaceRoot));

    const second = await client.execute("example.hello", "example.hello", {}, 42);
    assert.equal(second.id, 42);
    assert.equal(second.result.message, "Hello, world!");
    assert.equal(second.result.calls, 2);

    assert.match(client.stderr, /hello-node activated/);
    assert.match(client.stderr, /showMessage: Hello, Ada!/);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("plugin storage persists across host restarts", async (t) => {
  const fixture = await createFixture(t);
  let client = new HostClient(fixture.workspaceRoot);

  try {
    await initializeHost(client, fixture);
    const first = await client.execute("example.hello", "example.hello", {}, "first");
    assert.equal(first.result.calls, 1);

    const { response, exit } = await client.shutdown();
    assert.deepEqual(response.result, { ok: true });
    assert.equal(exit.code, 0);

    const storageFile = path.join(fixture.pluginDir, ".litecode-storage.json");
    const stored = JSON.parse(await fs.readFile(storageFile, "utf8"));
    assert.equal(stored.calls, 1);
    assert.equal(stored.deactivated, true);
  } finally {
    await client.close();
  }

  client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture);
    assert.deepEqual(result.errors, []);
    const second = await client.execute("example.hello", "example.hello", {}, "second");
    assert.equal(second.result.calls, 2);
  } finally {
    await client.close();
  }
});

test("stdin EOF deactivates plugins and exits", async (t) => {
  const fixture = await createFixture(t);
  const client = new HostClient(fixture.workspaceRoot);

  try {
    await initializeHost(client, fixture);
    await client.execute("example.hello", "example.hello", {}, "eof");

    client.child.stdin.end();
    const exit = await client.waitForExit(3000);
    assert.equal(exit.code, 0);

    const stored = JSON.parse(
      await fs.readFile(path.join(fixture.pluginDir, ".litecode-storage.json"), "utf8")
    );
    assert.equal(stored.deactivated, true);
  } finally {
    await client.close();
  }
});

test("unknown commands and handler failures return JSON-RPC errors", async (t) => {
  const fixture = await createFixture(t);
  const tools = await writePlugin(fixture.pluginsRoot, "test.tools", {
    commands: ["test.read", "test.fail", "test.bigint", "test.void"],
    source: `
      export function activate(ctx) {
        ctx.commands.register("test.read", (args) => ctx.workspace.readFile(args.path));
        ctx.commands.register("test.fail", () => {
          throw new Error("intentional failure");
        });
        ctx.commands.register("test.bigint", () => 1n);
        ctx.commands.register("test.void", () => undefined);
      }

      export function deactivate() {}
    `
  });

  await fs.writeFile(path.join(fixture.root, "outside.txt"), "secret", "utf8");

  const client = new HostClient(fixture.workspaceRoot);
  try {
    await initializeHost(client, fixture, [
      fixture.spec,
      { ...tools, main: path.join(tools.path, "index.mjs") }
    ]);

    const missing = await client.execute(
      "example.hello",
      "example.missing",
      undefined,
      "missing"
    );
    assert.equal(missing.id, "missing");
    assert.equal(missing.error.code, -32601);
    assert.equal(missing.error.data.pluginId, "example.hello");
    assert.equal(missing.error.data.command, "example.missing");

    const unknownPlugin = await client.execute("test.nope", "test.read", {}, "no-plugin");
    assert.equal(unknownPlugin.error.code, -32601);
    assert.equal(unknownPlugin.error.data.reason, "plugin-not-loaded");

    const inside = await client.execute("test.tools", "test.read", { path: "README.md" }, "inside");
    assert.equal(inside.result, fixture.readme);

    const outside = await client.execute(
      "test.tools",
      "test.read",
      { path: "../outside.txt" },
      "outside"
    );
    assert.equal(outside.id, "outside");
    assert.equal(outside.error.code, -32000);
    assert.match(outside.error.message, /outside the workspace/i);
    assert.equal(outside.error.data.pluginId, "test.tools");
    assert.equal(outside.error.data.command, "test.read");

    const absolute = await client.execute(
      "test.tools",
      "test.read",
      { path: path.join(fixture.root, "outside.txt") },
      "absolute"
    );
    assert.equal(absolute.error.code, -32000);
    assert.match(absolute.error.message, /outside the workspace/i);

    const failed = await client.execute("test.tools", "test.fail", {}, "failed");
    assert.equal(failed.error.code, -32000);
    assert.match(failed.error.message, /intentional failure/);
    assert.equal(failed.error.data.pluginId, "test.tools");
    assert.equal(failed.error.data.command, "test.fail");

    const bigint = await client.execute("test.tools", "test.bigint", {}, "bigint");
    assert.equal(bigint.error.code, -32000);
    assert.match(bigint.error.message, /JSON-serializable/i);

    const voidResult = await client.execute("test.tools", "test.void", {}, "void");
    assert.equal(voidResult.result, null);
  } finally {
    await client.close();
  }
});

test("tool/execute dispatches to registered tools with command fallback", async (t) => {
  const fixture = await createFixture(t);
  const agentTools = await writePlugin(fixture.pluginsRoot, "test.agent-tools", {
    commands: ["test.echo-command"],
    source: `
      export function activate(ctx) {
        ctx.commands.register("test.echo-command", (args) => ({ via: "command", args: args ?? {} }));
        ctx.tools.register("test.explicit", (args) => ({ via: "tool", doubled: (args?.n ?? 0) * 2 }));
        ctx.tools.register("test.broken", () => {
          throw new Error("tool exploded");
        });
      }

      export function deactivate() {}
    `
  });
  const undeclared = await writePlugin(fixture.pluginsRoot, "test.undeclared-tool", {
    commands: [],
    source: `
      export function activate(ctx) {
        ctx.tools.register("test.not-declared", () => null);
      }

      export function deactivate() {}
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      {
        ...agentTools,
        main: path.join(agentTools.path, "index.mjs"),
        tools: [
          { name: "test.explicit", command: "test.explicit" },
          { name: "test.fallback", command: "test.echo-command" },
          { name: "test.broken", command: "test.broken" }
        ]
      },
      undeclared
    ]);

    assert.equal(result.errors.length, 1);
    assert.equal(result.errors[0].pluginId, "test.undeclared-tool");
    assert.match(result.errors[0].message, /not declared/);
    assert.deepEqual(result.plugins, [
      {
        id: "example.hello",
        commands: ["example.hello"],
        tools: ["example.hello.tool"],
        views: [],
        uiViews: []
      },
      {
        id: "test.agent-tools",
        commands: ["test.echo-command"],
        tools: ["test.explicit", "test.fallback", "test.broken"],
        views: [],
        uiViews: []
      }
    ]);

    // ctx.tools.register 的 handler 优先。
    const explicit = await client.executeTool("test.agent-tools", "test.explicit", { n: 21 }, "t1");
    assert.equal(explicit.error, undefined);
    assert.deepEqual(explicit.result, { result: { via: "tool", doubled: 42 } });

    // 未注册 tools handler 时回退到清单 command 映射的命令 handler。
    const fallback = await client.executeTool("test.agent-tools", "test.fallback", { x: 1 }, "t2");
    assert.deepEqual(fallback.result, {
      result: { via: "command", args: { x: 1 } }
    });

    // 示例插件只注册了命令 handler，工具经回退执行同一 handler。
    const hello = await client.executeTool("example.hello", "example.hello.tool", { name: "Ada" }, "t3");
    assert.equal(hello.result.result.message, "Hello, Ada!");
    assert.equal(hello.result.result.calls, 1);

    const missing = await client.executeTool("example.hello", "test.nope", {}, "t4");
    assert.equal(missing.error.code, -32601);
    assert.equal(missing.error.data.reason, "tool-not-declared");

    const missingPlugin = await client.executeTool("test.nope", "test.explicit", {}, "t5");
    assert.equal(missingPlugin.error.code, -32601);
    assert.equal(missingPlugin.error.data.reason, "plugin-not-loaded");

    const broken = await client.executeTool("test.agent-tools", "test.broken", {}, "t6");
    assert.equal(broken.error.code, -32000);
    assert.match(broken.error.message, /tool exploded/);
    assert.equal(broken.error.data.tool, "test.broken");

    // 宿主在工具错误后仍可用。
    const stillWorks = await client.executeTool("test.agent-tools", "test.explicit", { n: 1 }, "t7");
    assert.equal(stillWorks.result.result.doubled, 2);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("malformed JSON and unknown methods use JSON-RPC error codes", async (t) => {
  const fixture = await createFixture(t);
  const client = new HostClient(fixture.workspaceRoot);

  try {
    await initializeHost(client, fixture);

    const parseWaiter = client.waitFor(
      (message) => message.error?.code === -32700,
      "parse error response"
    );
    client.sendRaw("{ this is not json");
    const parseError = await parseWaiter;
    assert.equal(parseError.id, null);
    assert.equal(parseError.jsonrpc, "2.0");

    const unknown = await client.request("does/not/exist", { hello: true }, "method-1");
    assert.equal(unknown.id, "method-1");
    assert.equal(unknown.error.code, -32601);
    assert.match(unknown.error.message, /does\/not\/exist/);

    const stillWorks = await client.execute(
      "example.hello",
      "example.hello",
      {},
      "after-error"
    );
    assert.equal(stillWorks.result.calls, 1);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("a failing plugin does not prevent other plugins from loading", async (t) => {
  const fixture = await createFixture(t);

  const brokenImport = await writePlugin(fixture.pluginsRoot, "test.broken-import", {
    main: "./missing.mjs",
    commands: [{ command: "test.broken", title: "Broken" }]
  });
  const brokenActivate = await writePlugin(fixture.pluginsRoot, "test.broken-activate", {
    commands: [{ command: "test.broken", title: "Broken" }],
    source: `
      export async function activate() {
        console.log("plugin stdout redirect check");
        throw new Error("activation exploded");
      }
    `
  });
  const undeclared = await writePlugin(fixture.pluginsRoot, "test.undeclared", {
    commands: [],
    source: `
      export function activate(ctx) {
        ctx.commands.register("test.not-declared", () => null);
      }
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      brokenImport,
      brokenActivate,
      undeclared
    ]);

    assert.equal(result.errors.length, 3);
    const errors = new Map(result.errors.map((error) => [error.pluginId, error]));

    assert.equal(errors.get("test.broken-import").stage, "import");
    assert.match(errors.get("test.broken-import").message, /missing\.mjs/);

    assert.equal(errors.get("test.broken-activate").stage, "activate");
    assert.match(errors.get("test.broken-activate").message, /activation exploded/);

    assert.equal(errors.get("test.undeclared").stage, "activate");
    assert.match(errors.get("test.undeclared").message, /not declared/);

    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello"]
    );

    const hello = await client.execute("example.hello", "example.hello", {}, "ok");
    assert.equal(hello.result.calls, 1);

    const gone = await client.execute("test.broken-activate", "test.broken", {}, "gone");
    assert.equal(gone.error.code, -32601);

    assert.deepEqual(client.protocolErrors, []);
    assert.match(client.stderr, /plugin stdout redirect check/);
  } finally {
    await client.close();
  }
});

test("invalid storage JSON is ignored and rewritten", async (t) => {
  const fixture = await createFixture(t);
  const storageFile = path.join(fixture.pluginDir, ".litecode-storage.json");
  await fs.writeFile(storageFile, "{ definitely-not-json", "utf8");

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture);
    assert.deepEqual(result.errors, []);

    const hello = await client.execute("example.hello", "example.hello", {}, "storage");
    assert.equal(hello.result.calls, 1);
    assert.match(client.stderr, /ignoring invalid JSON/);

    const stored = JSON.parse(await fs.readFile(storageFile, "utf8"));
    assert.equal(stored.calls, 1);
  } finally {
    await client.close();
  }
});

test("onCommand plugins lazy-activate on first execute; star and missing events stay eager", async (t) => {
  const fixture = await createFixture(t);
  const lazy = await writePlugin(fixture.pluginsRoot, "test.lazy", {
    commands: ["test.lazy.hello", "test.lazy.other"],
    tools: [{ name: "test.lazy.tool", command: "test.lazy.hello" }],
    activationEvents: ["onCommand:test.lazy.hello"],
    source: `
      export function activate(ctx) {
        ctx.log("test.lazy activated");
        ctx.commands.register("test.lazy.hello", (args) => ({ via: "command", args: args ?? {} }));
        ctx.commands.register("test.lazy.other", () => ({ via: "other" }));
      }
      export function deactivate() {
        console.error("test.lazy deactivated");
      }
    `
  });
  const lazyTool = await writePlugin(fixture.pluginsRoot, "test.lazy-tool", {
    commands: ["test.lazy-tool.run"],
    tools: [{ name: "test.lazy-tool.echo", command: "test.lazy-tool.run" }],
    activationEvents: ["onCommand:test.lazy-tool.run"],
    source: `
      export function activate(ctx) {
        ctx.log("test.lazy-tool activated");
        ctx.commands.register("test.lazy-tool.run", (args) => ({ via: "tool-fallback", args: args ?? {} }));
      }
    `
  });
  const eagerStar = await writePlugin(fixture.pluginsRoot, "test.eager-star", {
    commands: ["test.eager-star.run"],
    activationEvents: ["*"],
    source: `
      export function activate(ctx) {
        ctx.log("test.eager-star activated");
        ctx.commands.register("test.eager-star.run", () => "star-ok");
      }
    `
  });
  const never = await writePlugin(fixture.pluginsRoot, "test.never", {
    commands: ["test.never.run"],
    activationEvents: ["onCommand:test.never.run"],
    source: `
      export function activate(ctx) {
        ctx.log("test.never activated");
        ctx.commands.register("test.never.run", () => "never-ok");
      }
      export function deactivate() {
        console.error("test.never deactivate must not run");
      }
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      lazy,
      lazyTool,
      eagerStar,
      never
    ]);
    assert.deepEqual(result.errors, []);
    // initialize 只激活立即插件；延迟插件不 import/activate。
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello", "test.eager-star"]
    );
    assert.match(client.stderr, /hello-node activated/);
    assert.match(client.stderr, /test\.eager-star activated/);
    assert.doesNotMatch(client.stderr, /test\.lazy activated/);
    assert.doesNotMatch(client.stderr, /test\.lazy-tool activated/);
    assert.doesNotMatch(client.stderr, /test\.never activated/);

    // 命中未声明命令不触发激活，reason 保持 plugin-not-loaded。
    const undeclared = await client.execute("test.lazy", "test.undeclared", {}, "lazy-undeclared");
    assert.equal(undeclared.error.code, -32601);
    assert.equal(undeclared.error.data.reason, "plugin-not-loaded");
    assert.doesNotMatch(client.stderr, /test\.lazy activated/);

    // 首次 command/execute 懒激活并派发。
    const first = await client.execute("test.lazy", "test.lazy.hello", { n: 1 }, "lazy-first");
    assert.equal(first.error, undefined);
    assert.deepEqual(first.result, { via: "command", args: { n: 1 } });
    assert.match(client.stderr, /test\.lazy activated/);

    // tool/execute 命中另一延迟插件：懒激活后经 command 回退派发。
    const toolFirst = await client.executeTool("test.lazy-tool", "test.lazy-tool.echo", { n: 2 }, "lazy-tool");
    assert.equal(toolFirst.error, undefined);
    assert.deepEqual(toolFirst.result, {
      result: { via: "tool-fallback", args: { n: 2 } }
    });
    assert.match(client.stderr, /test\.lazy-tool activated/);

    // 幂等：重复 execute / tool 不再激活（各只激活一次）。
    const second = await client.execute("test.lazy", "test.lazy.other", {}, "lazy-second");
    assert.deepEqual(second.result, { via: "other" });
    const viaTool = await client.executeTool("test.lazy", "test.lazy.tool", { n: 3 }, "lazy-tool-again");
    assert.deepEqual(viaTool.result, { result: { via: "command", args: { n: 3 } } });
    assert.equal((client.stderr.match(/test\.lazy activated/g) ?? []).length, 1);
    assert.equal((client.stderr.match(/test\.lazy-tool activated/g) ?? []).length, 1);

    // shutdown：已激活的延迟插件正常 deactivate；从未激活的不调用 deactivate。
    const { response, exit } = await client.shutdown();
    assert.deepEqual(response.result, { ok: true });
    assert.equal(exit.code, 0);
    assert.match(client.stderr, /test\.lazy deactivated/);
    assert.doesNotMatch(client.stderr, /test\.never deactivate must not run/);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("lazy activation failure returns -32601 with reason and does not retry", async (t) => {
  const fixture = await createFixture(t);
  const brokenActivate = await writePlugin(fixture.pluginsRoot, "test.lazy-broken", {
    commands: ["test.lazy-broken.run"],
    activationEvents: ["onCommand:test.lazy-broken.run"],
    source: `
      export function activate() {
        console.error("test.lazy-broken activating");
        throw new Error("lazy activation exploded");
      }
    `
  });
  const brokenImport = await writePlugin(fixture.pluginsRoot, "test.lazy-import", {
    main: "./missing.mjs",
    commands: ["test.lazy-import.run"],
    activationEvents: ["onCommand:test.lazy-import.run"]
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      brokenActivate,
      brokenImport
    ]);
    // 延迟插件不在 initialize 加载，加载错误也不出现在 errors 里。
    assert.deepEqual(result.errors, []);
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello"]
    );
    assert.doesNotMatch(client.stderr, /test\.lazy-broken activating/);

    // activate 阶段失败：-32601 + reason/stage，stderr 记录。
    const first = await client.execute("test.lazy-broken", "test.lazy-broken.run", {}, "broken-1");
    assert.equal(first.error.code, -32601);
    assert.equal(first.error.data.reason, "activation-failed");
    assert.equal(first.error.data.stage, "activate");
    assert.match(first.error.message, /lazy activation exploded/);
    assert.match(client.stderr, /Lazy activation failed for plugin "test\.lazy-broken"/);
    assert.match(client.stderr, /test\.lazy-broken activating/);

    // 失败状态不重试：activate 只运行一次，错误原样返回。
    const second = await client.execute("test.lazy-broken", "test.lazy-broken.run", {}, "broken-2");
    assert.equal(second.error.code, -32601);
    assert.equal(second.error.data.reason, "activation-failed");
    assert.equal((client.stderr.match(/test\.lazy-broken activating/g) ?? []).length, 1);

    // import 阶段失败同样走 -32601 路径。
    const importFail = await client.execute("test.lazy-import", "test.lazy-import.run", {}, "broken-3");
    assert.equal(importFail.error.code, -32601);
    assert.equal(importFail.error.data.reason, "activation-failed");
    assert.equal(importFail.error.data.stage, "import");
    assert.match(importFail.error.message, /missing\.mjs/);

    // 其它插件不受影响。
    const hello = await client.execute("example.hello", "example.hello", {}, "broken-4");
    assert.equal(hello.result.calls, 1);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("view/children round-trips nodes, validates shapes and unknown views", async (t) => {
  const fixture = await createFixture(t);
  const tree = await writePlugin(fixture.pluginsRoot, "test.tree", {
    commands: [],
    views: ["test.tree", "test.bad-shape", "test.throwing"],
    source: `
      export function activate(ctx) {
        ctx.views.register("test.tree", {
          getChildren(nodeId) {
            if (nodeId === undefined) {
              return [
                { id: "a", label: "Alpha", icon: "file", command: "test.tree.open", tooltip: "open a", junk: 1 },
                { id: "b", label: "Beta" }
              ];
            }
            if (nodeId === "a") {
              return Promise.resolve([{ id: "a-1", label: "Child" }]);
            }
            return [];
          }
        });
        ctx.views.register("test.bad-shape", {
          getChildren() {
            return [{ id: "x" }, { id: "y", label: 42 }];
          }
        });
        ctx.views.register("test.throwing", {
          getChildren() {
            throw new Error("getChildren exploded");
          }
        });
      }
      export function deactivate() {}
    `
  });
  const nonArray = await writePlugin(fixture.pluginsRoot, "test.non-array-view", {
    commands: [],
    views: ["test.non-array"],
    source: `
      export function activate(ctx) {
        ctx.views.register("test.non-array", {
          getChildren() {
            return { not: "an array" };
          }
        });
      }
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      { ...tree, main: path.join(tree.path, "index.mjs") },
      { ...nonArray, main: path.join(nonArray.path, "index.mjs") }
    ]);
    assert.deepEqual(result.errors, []);
    assert.deepEqual(result.plugins.map((plugin) => plugin.id), [
      "example.hello",
      "test.tree",
      "test.non-array-view"
    ]);
    const treePlugin = result.plugins.find((plugin) => plugin.id === "test.tree");
    assert.deepEqual(treePlugin.views, ["test.tree", "test.bad-shape", "test.throwing"]);

    // 根节点：多余键被丢弃，icon/command/tooltip 原样保留。
    const roots = await client.viewChildren("test.tree", "test.tree", undefined, "v-root");
    assert.deepEqual(roots.result, [
      { id: "a", label: "Alpha", icon: "file", command: "test.tree.open", tooltip: "open a" },
      { id: "b", label: "Beta" }
    ]);

    // 节点展开：Promise provider 与空数组合法。
    const child = await client.viewChildren("test.tree", "test.tree", "a", "v-child");
    assert.deepEqual(child.result, [{ id: "a-1", label: "Child" }]);
    const leaf = await client.viewChildren("test.tree", "test.tree", "b", "v-leaf");
    assert.deepEqual(leaf.result, []);

    // 未声明的 viewId：-32601 带 reason。
    const undeclared = await client.viewChildren("test.tree", "test.undeclared", undefined, "v-none");
    assert.equal(undeclared.error.code, -32601);
    assert.equal(undeclared.error.data.pluginId, "test.tree");
    assert.equal(undeclared.error.data.viewId, "test.undeclared");
    assert.equal(undeclared.error.data.reason, "view-not-declared");

    // 未知插件：-32601 plugin-not-loaded。
    const unknownPlugin = await client.viewChildren("test.nope", "test.tree", undefined, "v-nope");
    assert.equal(unknownPlugin.error.code, -32601);
    assert.equal(unknownPlugin.error.data.reason, "plugin-not-loaded");

    // 形状非法：缺 label / label 非字符串 / 返回非数组 → -32000 reason=invalid-node。
    const badShape = await client.viewChildren("test.tree", "test.bad-shape", undefined, "v-bad");
    assert.equal(badShape.error.code, -32000);
    assert.equal(badShape.error.data.reason, "invalid-node");
    assert.match(badShape.error.message, /non-empty string label/);

    const notArray = await client.viewChildren("test.non-array-view", "test.non-array", undefined, "v-narr");
    assert.equal(notArray.error.code, -32000);
    assert.equal(notArray.error.data.reason, "invalid-node");

    // provider 抛错：-32000 带原始错误，宿主保持可用。
    const thrown = await client.viewChildren("test.tree", "test.throwing", undefined, "v-throw");
    assert.equal(thrown.error.code, -32000);
    assert.match(thrown.error.message, /getChildren exploded/);
    const afterThrow = await client.viewChildren("test.tree", "test.tree", undefined, "v-after");
    assert.equal(afterThrow.error, undefined);

    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("onView lazy-activates view providers; failures return -32601 without retry", async (t) => {
  const fixture = await createFixture(t);
  const lazyView = await writePlugin(fixture.pluginsRoot, "test.lazy-view", {
    commands: ["test.lazy-view.run"],
    views: ["test.lazy-view.tree"],
    activationEvents: ["onView:test.lazy-view.tree"],
    source: `
      export function activate(ctx) {
        ctx.log("test.lazy-view activated");
        ctx.views.register("test.lazy-view.tree", {
          getChildren(nodeId) {
            return [{ id: "root", label: nodeId ?? "lazy root", command: "test.lazy-view.run" }];
          }
        });
      }
      export function deactivate() {
        console.error("test.lazy-view deactivated");
      }
    `
  });
  const brokenView = await writePlugin(fixture.pluginsRoot, "test.broken-view", {
    commands: ["test.broken-view.run"],
    views: ["test.broken-view.tree"],
    activationEvents: ["onView:test.broken-view.tree"],
    source: `
      export function activate() {
        console.error("test.broken-view activating");
        throw new Error("view activation exploded");
      }
    `
  });
  const eagerView = await writePlugin(fixture.pluginsRoot, "test.eager-view", {
    commands: [],
    views: ["test.eager-view.tree"],
    activationEvents: ["*"],
    source: `
      export function activate(ctx) {
        ctx.log("test.eager-view activated");
        // 故意不注册 provider：view-not-registered。
      }
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      lazyView,
      brokenView,
      eagerView
    ]);
    assert.deepEqual(result.errors, []);
    // 立即插件只有 eager-view 与示例插件；onView 插件不在 initialize 加载。
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello", "test.eager-view"]
    );
    assert.doesNotMatch(client.stderr, /test\.lazy-view activated/);

    // view/children 命中延迟插件声明的视图：懒激活后返回节点。
    const first = await client.viewChildren("test.lazy-view", "test.lazy-view.tree", undefined, "lv-1");
    assert.equal(first.error, undefined);
    assert.deepEqual(first.result, [
      { id: "root", label: "lazy root", command: "test.lazy-view.run" }
    ]);
    assert.match(client.stderr, /test\.lazy-view activated/);

    // nodeId 透传 provider。
    const nested = await client.viewChildren("test.lazy-view", "test.lazy-view.tree", "n1", "lv-2");
    assert.deepEqual(nested.result, [{ id: "root", label: "n1", command: "test.lazy-view.run" }]);

    // 幂等：重复请求不再次激活。
    await client.viewChildren("test.lazy-view", "test.lazy-view.tree", undefined, "lv-3");
    assert.equal((client.stderr.match(/test\.lazy-view activated/g) ?? []).length, 1);

    // activate 失败的延迟插件：-32601 + reason/stage，且不重试。
    const broken = await client.viewChildren("test.broken-view", "test.broken-view.tree", undefined, "bv-1");
    assert.equal(broken.error.code, -32601);
    assert.equal(broken.error.data.reason, "activation-failed");
    assert.equal(broken.error.data.stage, "activate");
    assert.match(broken.error.message, /view activation exploded/);
    const brokenAgain = await client.viewChildren("test.broken-view", "test.broken-view.tree", undefined, "bv-2");
    assert.equal(brokenAgain.error.code, -32601);
    assert.equal((client.stderr.match(/test\.broken-view activating/g) ?? []).length, 1);

    // 立即插件未注册 provider：-32601 view-not-registered。
    const eager = await client.viewChildren("test.eager-view", "test.eager-view.tree", undefined, "ev-1");
    assert.equal(eager.error.code, -32601);
    assert.equal(eager.error.data.reason, "view-not-registered");

    // shutdown：已激活的延迟视图插件正常 deactivate。
    const { response, exit } = await client.shutdown();
    assert.deepEqual(response.result, { ok: true });
    assert.equal(exit.code, 0);
    assert.match(client.stderr, /test\.lazy-view deactivated/);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("view/children provider timeout returns -32000 and host stays responsive", async (t) => {
  const fixture = await createFixture(t);
  const hanging = await writePlugin(fixture.pluginsRoot, "test.hanging-view", {
    commands: [],
    views: ["test.hanging-view.tree"],
    source: `
      export function activate(ctx) {
        ctx.views.register("test.hanging-view.tree", {
          getChildren() {
            return new Promise(() => {});
          }
        });
      }
    `
  });

  // 短超时注入：默认 30s，测试用环境变量压到 150ms。
  const client = new HostClient(fixture.workspaceRoot, {
    env: { LC_EXT_HOST_VIEW_TIMEOUT_MS: "150" }
  });
  try {
    await initializeHost(client, fixture, [
      fixture.spec,
      { ...hanging, main: path.join(hanging.path, "index.mjs") }
    ]);

    const timedOut = await client.viewChildren(
      "test.hanging-view",
      "test.hanging-view.tree",
      undefined,
      "vt-1"
    );
    assert.equal(timedOut.error.code, -32000);
    assert.equal(timedOut.error.data.reason, "provider-timeout");
    assert.match(timedOut.error.message, /timed out after 150ms/);

    // 宿主在超时后仍可用（示例插件命令照常执行）。
    const hello = await client.execute("example.hello", "example.hello", {}, "vt-2");
    assert.equal(hello.result.calls, 1);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("ui/values round-trips per-section values by stable id and validates shapes", async (t) => {
  const fixture = await createFixture(t);
  const board = await writePlugin(fixture.pluginsRoot, "test.board", {
    commands: [],
    uiViews: [
      {
        id: "test.board",
        sections: [
          { id: "intro", kind: "markdown" },
          {
            id: "filter",
            kind: "form",
            fields: [
              { key: "q", type: "text" },
              { key: "limit", type: "number", min: 1, max: 200, default: 50 },
              { key: "done", type: "boolean", default: false },
              { key: "sort", type: "select", options: ["new", "old"], default: "new" },
              { key: "token", type: "password" }
            ]
          },
          {
            id: "tasks",
            kind: "list",
            source: "provider",
            columns: [{ key: "title" }, { key: "state" }]
          },
          { id: "stats", kind: "kv", source: "provider" },
          { id: "static-stats", kind: "kv" }
        ]
      }
    ],
    source: `
      export function activate(ctx) {
        ctx.ui.register("test.board", {
          getValues(sectionId) {
            ctx.log("board getValues " + sectionId);
            if (sectionId === "tasks") {
              return {
                rows: [
                  { title: "Alpha", state: 1, junk: { x: 1 }, extra: "dropped" },
                  { title: "Beta", state: false },
                  { title: "Gamma" }
                ]
              };
            }
            if (sectionId === "stats") {
              return { items: [{ key: "总数", value: "42", extra: 1 }] };
            }
            return { anything: true };
          }
        });
      }
      export function deactivate() {}
    `
  });
  const badValues = await writePlugin(fixture.pluginsRoot, "test.bad-values", {
    commands: [],
    uiViews: [
      {
        id: "test.bad-values",
        sections: [
          { id: "not-rows", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "bad-row", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "many-rows", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "cell-object", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "cell-array", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "cell-null", kind: "list", source: "provider", columns: [{ key: "a" }] },
          { id: "not-items", kind: "kv", source: "provider" },
          { id: "missing-key", kind: "kv", source: "provider" },
          { id: "empty-key", kind: "kv", source: "provider" },
          { id: "non-string-value", kind: "kv", source: "provider" },
          { id: "many-items", kind: "kv", source: "provider" },
          { id: "throwing", kind: "kv", source: "provider" }
        ]
      }
    ],
    source: `
      export function activate(ctx) {
        ctx.ui.register("test.bad-values", {
          getValues(sectionId) {
            switch (sectionId) {
              case "not-rows":
                return { values: {} };
              case "bad-row":
                return { rows: ["nope"] };
              case "many-rows":
                return { rows: Array(501).fill({ a: 1 }) };
              case "cell-object":
                return { rows: [{ a: { nested: true } }] };
              case "cell-array":
                return { rows: [{ a: [1, 2] }] };
              case "cell-null":
                return { rows: [{ a: null }] };
              case "not-items":
                return { rows: [] };
              case "missing-key":
                return { items: [{ value: "v" }] };
              case "empty-key":
                return { items: [{ key: "", value: "v" }] };
              case "non-string-value":
                return { items: [{ key: "k", value: 42 }] };
              case "many-items":
                return { items: Array(65).fill({ key: "k", value: "v" }) };
              case "throwing":
                throw new Error("getValues exploded");
              default:
                return null;
            }
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
      { ...board, main: path.join(board.path, "index.mjs") },
      { ...badValues, main: path.join(badValues.path, "index.mjs") }
    ]);
    assert.deepEqual(result.errors, []);
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello", "test.board", "test.bad-values"]
    );
    // initialize 结果回显完整节描述 DTO（id/kind/source/fields/columns 全保留）。
    const boardPlugin = result.plugins.find((plugin) => plugin.id === "test.board");
    assert.deepEqual(boardPlugin.uiViews, [
      {
        id: "test.board",
        sections: [
          { id: "intro", kind: "markdown" },
          {
            id: "filter",
            kind: "form",
            fields: [
              { key: "q", type: "text" },
              { key: "limit", type: "number", min: 1, max: 200, default: 50 },
              { key: "done", type: "boolean", default: false },
              { key: "sort", type: "select", options: ["new", "old"], default: "new" },
              { key: "token", type: "password" }
            ]
          },
          { id: "tasks", kind: "list", source: "provider", columns: [{ key: "title" }, { key: "state" }] },
          { id: "stats", kind: "kv", source: "provider" },
          { id: "static-stats", kind: "kv" }
        ]
      }
    ]);
    const helloPlugin = result.plugins.find((plugin) => plugin.id === "example.hello");
    assert.deepEqual(helloPlugin.uiViews, []);

    // list 节按稳定 id 派发：provider 收到 "tasks"；未知列丢弃、缺列不补。
    const list = await client.uiValues("test.board", "test.board", "tasks", "u-list");
    assert.deepEqual(list.result, {
      rows: [{ title: "Alpha", state: 1 }, { title: "Beta", state: false }, { title: "Gamma" }]
    });
    assert.match(client.stderr, /board getValues tasks/);

    // kv 节：条目白名单归一，多余键丢弃。
    const kv = await client.uiValues("test.board", "test.board", "stats", "u-kv");
    assert.deepEqual(kv.result, { items: [{ key: "总数", value: "42" }] });

    // markdown 节无回读：-32000 reason=invalid-values，provider 零调用。
    const markdown = await client.uiValues("test.board", "test.board", "intro", "u-md");
    assert.equal(markdown.error.code, -32000);
    assert.equal(markdown.error.data.reason, "invalid-values");
    assert.equal(markdown.error.data.sectionId, "intro");
    assert.doesNotMatch(client.stderr, /board getValues intro/);

    // form 节无回读：invalid-values，provider 零调用。
    const form = await client.uiValues("test.board", "test.board", "filter", "u-form");
    assert.equal(form.error.code, -32000);
    assert.equal(form.error.data.reason, "invalid-values");
    assert.doesNotMatch(client.stderr, /board getValues filter/);

    // 无 source:"provider" 的静态 kv 节：invalid-values，provider 零调用。
    const staticKv = await client.uiValues("test.board", "test.board", "static-stats", "u-static");
    assert.equal(staticKv.error.code, -32000);
    assert.equal(staticKv.error.data.reason, "invalid-values");
    assert.doesNotMatch(client.stderr, /board getValues static-stats/);

    // 未知节（旧序号语义不再接受）：invalid-values，provider 零调用。
    const unknown = await client.uiValues("test.board", "test.board", "0", "u-oob");
    assert.equal(unknown.error.code, -32000);
    assert.equal(unknown.error.data.reason, "invalid-values");
    const numeric = await client.uiValues("test.board", "test.board", "3", "u-nan");
    assert.equal(numeric.error.code, -32000);
    assert.equal(numeric.error.data.reason, "invalid-values");
    assert.equal((client.stderr.match(/board getValues/g) ?? []).length, 2);

    // 未声明视图 / 未知插件：-32601 带 reason。
    const undeclared = await client.uiValues("test.board", "test.undeclared", "tasks", "u-none");
    assert.equal(undeclared.error.code, -32601);
    assert.equal(undeclared.error.data.reason, "view-not-declared");
    const unknownPlugin = await client.uiValues("test.nope", "test.board", "tasks", "u-nope");
    assert.equal(unknownPlugin.error.code, -32601);
    assert.equal(unknownPlugin.error.data.reason, "plugin-not-loaded");

    // 形状非法全集：-32000 reason=invalid-values。
    const badShapeCases = [
      ["not-rows", /must return \{ rows/],
      ["bad-row", /invalid row at index 0/],
      ["many-rows", /more than 500 rows/],
      ["cell-object", /invalid value in column "a"/],
      ["cell-array", /invalid value in column "a"/],
      ["cell-null", /invalid value in column "a"/],
      ["not-items", /must return \{ items/],
      ["missing-key", /non-empty string key/],
      ["empty-key", /non-empty string key/],
      ["non-string-value", /without a string value/],
      ["many-items", /more than 64 items/]
    ];
    for (let index = 0; index < badShapeCases.length; index++) {
      const [sectionId, pattern] = badShapeCases[index];
      const response = await client.uiValues("test.bad-values", "test.bad-values", sectionId, `u-bad-${index}`);
      assert.equal(response.error.code, -32000, sectionId);
      assert.equal(response.error.data.reason, "invalid-values", sectionId);
      assert.match(response.error.message, pattern, sectionId);
    }

    // provider 抛错：-32000 带原始错误，宿主保持可用。
    const thrown = await client.uiValues("test.bad-values", "test.bad-values", "throwing", "u-throw");
    assert.equal(thrown.error.code, -32000);
    assert.match(thrown.error.message, /getValues exploded/);
    const afterThrow = await client.uiValues("test.board", "test.board", "stats", "u-after");
    assert.equal(afterThrow.error, undefined);

    // 参数形状校验。
    const noSection = await client.request(
      "ui/values",
      { pluginId: "test.board", viewId: "test.board" },
      "u-nosec"
    );
    assert.equal(noSection.error.code, -32602);

    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("ctx.ui.register is restricted to declared ui views and payload shapes", async (t) => {
  const fixture = await createFixture(t);
  const undeclared = await writePlugin(fixture.pluginsRoot, "test.undeclared-ui", {
    commands: [],
    uiViews: [],
    source: `
      export function activate(ctx) {
        ctx.ui.register("test.not-declared", { getValues: () => ({}) });
      }
    `
  });
  const malformed = await writePlugin(fixture.pluginsRoot, "test.malformed-ui", {
    commands: [],
    uiViews: [{ id: "test.malformed" }],
    source: `
      export function activate(ctx) {}
    `
  });
  const legacy = await writePlugin(fixture.pluginsRoot, "test.legacy-ui", {
    commands: [],
    uiViews: [{ id: "test.legacy", sectionKinds: ["kv"] }],
    source: `
      export function activate(ctx) {}
    `
  });
  const badKind = await writePlugin(fixture.pluginsRoot, "test.bad-kind-ui", {
    commands: [],
    uiViews: [{ id: "test.bad-kind", sections: [{ id: "s", kind: "chart" }] }],
    source: `
      export function activate(ctx) {}
    `
  });
  const badSource = await writePlugin(fixture.pluginsRoot, "test.bad-source-ui", {
    commands: [],
    uiViews: [
      {
        id: "test.bad-source",
        sections: [{ id: "s", kind: "list", columns: [{ key: "a" }] }]
      }
    ],
    source: `
      export function activate(ctx) {}
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      { ...undeclared, main: path.join(undeclared.path, "index.mjs") },
      { ...malformed, main: path.join(malformed.path, "index.mjs") },
      { ...legacy, main: path.join(legacy.path, "index.mjs") },
      { ...badKind, main: path.join(badKind.path, "index.mjs") },
      { ...badSource, main: path.join(badSource.path, "index.mjs") }
    ]);
    assert.equal(result.errors.length, 5);
    const errors = new Map(result.errors.map((error) => [error.pluginId, error]));
    assert.equal(errors.get("test.undeclared-ui").stage, "activate");
    assert.match(errors.get("test.undeclared-ui").message, /not declared/);
    assert.equal(errors.get("test.malformed-ui").stage, "manifest");
    assert.match(errors.get("test.malformed-ui").message, /must declare sections/);
    // 旧 sectionKinds 载荷明确拒绝：插件级 manifest 错误，不做猜测转换。
    assert.equal(errors.get("test.legacy-ui").stage, "manifest");
    assert.match(errors.get("test.legacy-ui").message, /sectionKinds/);
    // 节 kind/source 闭集校验（宿主防御层，Rust 校验期为第一道）。
    assert.equal(errors.get("test.bad-kind-ui").stage, "manifest");
    assert.match(errors.get("test.bad-kind-ui").message, /invalid kind/);
    assert.equal(errors.get("test.bad-source-ui").stage, "manifest");
    assert.match(errors.get("test.bad-source-ui").message, /source "provider"/);
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello"]
    );

    // 注册失败的插件不在注册表：-32601 plugin-not-loaded。
    const missing = await client.uiValues("test.undeclared-ui", "test.not-declared", "s", "ru-1");
    assert.equal(missing.error.code, -32601);
    assert.equal(missing.error.data.reason, "plugin-not-loaded");
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("onUiView lazy-activates ui providers; failures return -32601 without retry", async (t) => {
  const fixture = await createFixture(t);
  const lazyBoard = await writePlugin(fixture.pluginsRoot, "test.lazy-board", {
    commands: ["test.lazy-board.run"],
    uiViews: [
      {
        id: "test.lazy-board.board",
        sections: [
          { id: "filter", kind: "form", fields: [{ key: "q", type: "text" }] },
          { id: "tasks", kind: "list", source: "provider", columns: [{ key: "title" }] }
        ]
      }
    ],
    activationEvents: ["onUiView:test.lazy-board.board"],
    source: `
      export function activate(ctx) {
        ctx.log("test.lazy-board activated");
        ctx.ui.register("test.lazy-board.board", {
          getValues(sectionId) {
            ctx.log("lazy-board getValues " + sectionId);
            if (sectionId === "tasks") return { rows: [{ title: "lazy row" }] };
            return { anything: true };
          }
        });
      }
      export function deactivate() {
        console.error("test.lazy-board deactivated");
      }
    `
  });
  const brokenBoard = await writePlugin(fixture.pluginsRoot, "test.broken-board", {
    commands: ["test.broken-board.run"],
    uiViews: [
      {
        id: "test.broken-board.board",
        sections: [{ id: "items", kind: "kv", source: "provider" }]
      }
    ],
    activationEvents: ["onUiView:test.broken-board.board"],
    source: `
      export function activate() {
        console.error("test.broken-board activating");
        throw new Error("ui activation exploded");
      }
    `
  });
  const eagerBoard = await writePlugin(fixture.pluginsRoot, "test.eager-board", {
    commands: [],
    uiViews: [
      {
        id: "test.eager-board.board",
        sections: [{ id: "items", kind: "kv", source: "provider" }]
      }
    ],
    activationEvents: ["*"],
    source: `
      export function activate(ctx) {
        ctx.log("test.eager-board activated");
        // 故意不注册 provider：view-not-registered。
      }
    `
  });

  const client = new HostClient(fixture.workspaceRoot);
  try {
    const result = await initializeHost(client, fixture, [
      fixture.spec,
      { ...lazyBoard, main: path.join(lazyBoard.path, "index.mjs") },
      { ...brokenBoard, main: path.join(brokenBoard.path, "index.mjs") },
      { ...eagerBoard, main: path.join(eagerBoard.path, "index.mjs") }
    ]);
    assert.deepEqual(result.errors, []);
    // 立即插件只有 eager-board 与示例插件；onUiView 插件不在 initialize 加载。
    assert.deepEqual(
      result.plugins.map((plugin) => plugin.id),
      ["example.hello", "test.eager-board"]
    );
    assert.doesNotMatch(client.stderr, /test\.lazy-board activated/);

    // ui/values 命中延迟插件声明的视图：懒激活后按稳定节 id 派发并归一。
    const first = await client.uiValues(
      "test.lazy-board",
      "test.lazy-board.board",
      "tasks",
      "lu-1"
    );
    assert.equal(first.error, undefined);
    assert.deepEqual(first.result, { rows: [{ title: "lazy row" }] });
    assert.match(client.stderr, /test\.lazy-board activated/);
    assert.match(client.stderr, /lazy-board getValues tasks/);

    // form 节无回读：invalid-values，provider 零调用。
    const form = await client.uiValues(
      "test.lazy-board",
      "test.lazy-board.board",
      "filter",
      "lu-form"
    );
    assert.equal(form.error.code, -32000);
    assert.equal(form.error.data.reason, "invalid-values");
    assert.doesNotMatch(client.stderr, /lazy-board getValues filter/);

    // 幂等：重复请求不再次激活。
    await client.uiValues("test.lazy-board", "test.lazy-board.board", "tasks", "lu-3");
    assert.equal((client.stderr.match(/test\.lazy-board activated/g) ?? []).length, 1);

    // activate 失败的延迟插件：-32601 + reason/stage，且不重试。
    const broken = await client.uiValues(
      "test.broken-board",
      "test.broken-board.board",
      "items",
      "bu-1"
    );
    assert.equal(broken.error.code, -32601);
    assert.equal(broken.error.data.reason, "activation-failed");
    assert.equal(broken.error.data.stage, "activate");
    assert.match(broken.error.message, /ui activation exploded/);
    const brokenAgain = await client.uiValues(
      "test.broken-board",
      "test.broken-board.board",
      "items",
      "bu-2"
    );
    assert.equal(brokenAgain.error.code, -32601);
    assert.equal((client.stderr.match(/test\.broken-board activating/g) ?? []).length, 1);

    // 立即插件未注册 provider：-32601 view-not-registered。
    const eager = await client.uiValues("test.eager-board", "test.eager-board.board", "items", "eu-1");
    assert.equal(eager.error.code, -32601);
    assert.equal(eager.error.data.reason, "view-not-registered");

    // shutdown：已激活的延迟 UI 插件正常 deactivate。
    const { response, exit } = await client.shutdown();
    assert.deepEqual(response.result, { ok: true });
    assert.equal(exit.code, 0);
    assert.match(client.stderr, /test\.lazy-board deactivated/);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("ui/values provider timeout returns -32000 and host stays responsive", async (t) => {
  const fixture = await createFixture(t);
  const hanging = await writePlugin(fixture.pluginsRoot, "test.hanging-ui", {
    commands: [],
    uiViews: [
      {
        id: "test.hanging-ui.board",
        sections: [{ id: "items", kind: "kv", source: "provider" }]
      }
    ],
    source: `
      export function activate(ctx) {
        ctx.ui.register("test.hanging-ui.board", {
          getValues() {
            return new Promise(() => {});
          }
        });
      }
    `
  });

  // 短超时注入：与 view/children 共用 LC_EXT_HOST_VIEW_TIMEOUT_MS。
  const client = new HostClient(fixture.workspaceRoot, {
    env: { LC_EXT_HOST_VIEW_TIMEOUT_MS: "150" }
  });
  try {
    await initializeHost(client, fixture, [
      fixture.spec,
      { ...hanging, main: path.join(hanging.path, "index.mjs") }
    ]);

    const timedOut = await client.uiValues(
      "test.hanging-ui",
      "test.hanging-ui.board",
      "items",
      "ut-1"
    );
    assert.equal(timedOut.error.code, -32000);
    assert.equal(timedOut.error.data.reason, "provider-timeout");
    assert.match(timedOut.error.message, /timed out after 150ms/);

    // 宿主在超时后仍可用（示例插件命令照常执行）。
    const hello = await client.execute("example.hello", "example.hello", {}, "ut-2");
    assert.equal(hello.result.calls, 1);
    assert.deepEqual(client.protocolErrors, []);
  } finally {
    await client.close();
  }
});

test("generated asset matches sidecar sources", async () => {
  const { stdout } = await execFileAsync(process.execPath, [BUILD_SCRIPT, "--check"], {
    cwd: REPO_ROOT
  });
  assert.match(stdout, /is up to date/);
});
