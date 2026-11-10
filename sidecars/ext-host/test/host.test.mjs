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
      { id: "example.hello", commands: ["example.hello"], tools: ["example.hello.tool"], views: [] }
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
      { id: "example.hello", commands: ["example.hello"], tools: ["example.hello.tool"], views: [] },
      {
        id: "test.agent-tools",
        commands: ["test.echo-command"],
        tools: ["test.explicit", "test.fallback", "test.broken"],
        views: []
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

test("generated asset matches sidecar sources", async () => {
  const { stdout } = await execFileAsync(process.execPath, [BUILD_SCRIPT, "--check"], {
    cwd: REPO_ROOT
  });
  assert.match(stdout, /is up to date/);
});
