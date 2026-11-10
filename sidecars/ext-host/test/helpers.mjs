import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const testDir = path.dirname(fileURLToPath(import.meta.url));

export const REPO_ROOT = path.resolve(testDir, "..", "..", "..");
export const ASSET_PATH = path.join(REPO_ROOT, "crates", "lc-core", "assets", "ext-host.mjs");
export const BUILD_SCRIPT = path.join(REPO_ROOT, "sidecars", "ext-host", "build.mjs");

const REQUEST_TIMEOUT_MS = 5000;

export class HostClient {
  constructor(cwd, { env } = {}) {
    this.child = spawn(process.execPath, [ASSET_PATH], {
      cwd,
      stdio: ["pipe", "pipe", "pipe"],
      env: env ? { ...process.env, ...env } : undefined
    });
    this.buffer = "";
    this.stderr = "";
    this.protocolErrors = [];
    this.messages = [];
    this.waiters = [];
    this.exitWaiters = [];
    this.exitInfo = null;
    this.nextId = 1;

    this.child.stdout.setEncoding("utf8");
    this.child.stderr.setEncoding("utf8");
    this.child.stdout.on("data", (chunk) => this.readStdout(chunk));
    this.child.stderr.on("data", (chunk) => {
      this.stderr += chunk;
    });
    this.child.on("exit", (code, signal) => {
      this.exitInfo = { code, signal };
      for (const waiter of this.exitWaiters.splice(0)) {
        clearTimeout(waiter.timer);
        waiter.resolve(this.exitInfo);
      }
      this.failWaiters(
        new Error(
          `ext-host exited before responding (code=${code}, signal=${signal})\nstderr:\n${this.stderr}`
        )
      );
    });
    this.child.stdin.on("error", () => {});
  }

  readStdout(chunk) {
    this.buffer += chunk;
    let newlineIndex;
    while ((newlineIndex = this.buffer.indexOf("\n")) !== -1) {
      const line = this.buffer.slice(0, newlineIndex).replace(/\r$/, "");
      this.buffer = this.buffer.slice(newlineIndex + 1);
      if (line.trim() === "") continue;
      try {
        this.deliver(JSON.parse(line));
      } catch {
        this.protocolErrors.push(line);
        this.failWaiters(
          new Error(`ext-host wrote non-JSON stdout: ${line}\nstderr:\n${this.stderr}`)
        );
      }
    }
  }

  deliver(message) {
    const index = this.waiters.findIndex((waiter) => {
      try {
        return waiter.predicate(message);
      } catch {
        return false;
      }
    });
    if (index === -1) {
      this.messages.push(message);
      return;
    }
    const [waiter] = this.waiters.splice(index, 1);
    clearTimeout(waiter.timer);
    waiter.resolve(message);
  }

  failWaiters(error) {
    for (const waiter of this.waiters.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
  }

  waitFor(predicate, description = "response", timeoutMs = REQUEST_TIMEOUT_MS) {
    if (this.protocolErrors.length > 0) {
      return Promise.reject(
        new Error(`ext-host wrote non-JSON stdout: ${this.protocolErrors.join("\n")}`)
      );
    }

    const index = this.messages.findIndex(predicate);
    if (index !== -1) {
      const [message] = this.messages.splice(index, 1);
      return Promise.resolve(message);
    }

    return new Promise((resolve, reject) => {
      const waiter = { predicate, resolve, reject, timer: null };
      waiter.timer = setTimeout(() => {
        const indexNow = this.waiters.indexOf(waiter);
        if (indexNow !== -1) this.waiters.splice(indexNow, 1);
        reject(
          new Error(
            `Timed out waiting for ${description}\nstderr:\n${this.stderr}\nprotocol errors:\n${this.protocolErrors.join("\n")}`
          )
        );
      }, timeoutMs);
      this.waiters.push(waiter);
    });
  }

  waitForExit(timeoutMs = REQUEST_TIMEOUT_MS) {
    if (this.exitInfo) return Promise.resolve(this.exitInfo);
    return new Promise((resolve, reject) => {
      const waiter = { resolve, timer: null };
      waiter.timer = setTimeout(() => {
        const index = this.exitWaiters.indexOf(waiter);
        if (index !== -1) this.exitWaiters.splice(index, 1);
        reject(new Error(`Timed out waiting for ext-host to exit\nstderr:\n${this.stderr}`));
      }, timeoutMs);
      this.exitWaiters.push(waiter);
    });
  }

  send(message) {
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  sendRaw(line) {
    this.child.stdin.write(line.endsWith("\n") ? line : `${line}\n`);
  }

  request(method, params, id = this.nextId++) {
    const response = this.waitFor(
      (message) =>
        message.id === id && (Object.hasOwn(message, "result") || Object.hasOwn(message, "error")),
      `${method} response (id=${JSON.stringify(id)})`
    );
    const message = { jsonrpc: "2.0", id, method };
    if (params !== undefined) message.params = params;
    this.send(message);
    return response;
  }

  execute(pluginId, command, args, id) {
    return this.request("command/execute", { pluginId, command, args }, id);
  }

  executeTool(pluginId, tool, args, id) {
    return this.request("tool/execute", { pluginId, tool, args }, id);
  }

  viewChildren(pluginId, viewId, nodeId, id) {
    const params = { pluginId, viewId };
    if (nodeId !== undefined && nodeId !== null) params.nodeId = nodeId;
    return this.request("view/children", params, id);
  }

  initialize(workspaceRoot, plugins, id = "initialize") {
    return this.request("initialize", { workspaceRoot, plugins }, id);
  }

  async shutdown() {
    const response = await this.request("shutdown", {}, "shutdown");
    const exit = await this.waitForExit();
    return { response, exit };
  }

  async close() {
    if (this.exitInfo) return;
    try {
      this.child.stdin.end();
    } catch {
      // The pipe may already be closed.
    }
    try {
      await this.waitForExit(1000);
      return;
    } catch {
      // Fall through to a hard kill.
    }
    this.child.kill();
    try {
      await this.waitForExit(2000);
    } catch {
      // Nothing else can be done during test cleanup.
    }
  }
}

export async function createFixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "lc-ext-host-"));
  const workspaceRoot = path.join(root, "workspace");
  const pluginsRoot = path.join(root, "plugins");
  const pluginDir = path.join(pluginsRoot, "hello-node");

  await fs.mkdir(workspaceRoot, { recursive: true });
  await fs.mkdir(pluginsRoot, { recursive: true });
  await fs.cp(path.join(REPO_ROOT, "examples", "plugins", "hello-node"), pluginDir, {
    recursive: true
  });

  const readme =
    "# Lite Code ext-host test workspace\n\nThis README is deliberately longer than eighty characters so the example plugin truncates it.\n";
  await fs.writeFile(path.join(workspaceRoot, "README.md"), readme, "utf8");
  await fs.writeFile(path.join(pluginDir, ".litecode-storage.json"), "{}\n", "utf8");

  const manifest = JSON.parse(await fs.readFile(path.join(pluginDir, "plugin.json"), "utf8"));
  const spec = {
    id: manifest.id,
    path: pluginDir,
    main: manifest.main,
    commands: manifest.contributes.commands,
    tools: manifest.contributes.tools ?? [],
    views: manifest.contributes.views ?? []
  };

  t.after(async () => {
    await fs.rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  });

  return { root, workspaceRoot, pluginsRoot, pluginDir, manifest, spec, readme };
}

export async function writePlugin(
  pluginsRoot,
  id,
  { main = "./index.mjs", source, commands = [], tools, views, activationEvents } = {}
) {
  const dir = path.join(pluginsRoot, id);
  await fs.mkdir(dir, { recursive: true });
  if (source !== undefined) {
    await fs.writeFile(path.join(dir, "index.mjs"), source, "utf8");
  }
  const spec = { id, path: dir, main, commands };
  if (tools !== undefined) spec.tools = tools;
  if (views !== undefined) spec.views = views;
  if (activationEvents !== undefined) spec.activationEvents = activationEvents;
  return spec;
}

export async function initializeHost(client, fixture, plugins = [fixture.spec]) {
  const response = await client.initialize(fixture.workspaceRoot, plugins);
  if (response.error) {
    throw new Error(`initialize failed: ${JSON.stringify(response.error)}`);
  }
  return response.result;
}
