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
      { id: "example.hello", commands: ["example.hello"], tools: ["example.hello.tool"] }
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
      { id: "example.hello", commands: ["example.hello"], tools: ["example.hello.tool"] },
      {
        id: "test.agent-tools",
        commands: ["test.echo-command"],
        tools: ["test.explicit", "test.fallback", "test.broken"]
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

test("generated asset matches sidecar sources", async () => {
  const { stdout } = await execFileAsync(process.execPath, [BUILD_SCRIPT, "--check"], {
    cwd: REPO_ROOT
  });
  assert.match(stdout, /is up to date/);
});
