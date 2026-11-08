let activeContext = null;

export async function activate(ctx) {
  activeContext = ctx;
  ctx.log("hello-node activated");

  ctx.commands.register("example.hello", async (args) => {
    const input = args && typeof args === "object" ? args : {};
    const name =
      typeof input.name === "string" && input.name.trim() !== ""
        ? input.name.trim()
        : "world";

    let readme = null;
    try {
      readme = (await ctx.workspace.readFile("README.md")).slice(0, 80);
    } catch (error) {
      if (!error || error.code !== "ENOENT") throw error;
    }

    const previous = ctx.storage.get("calls");
    const calls = (typeof previous === "number" ? previous : 0) + 1;
    ctx.storage.set("calls", calls);

    const message = `Hello, ${name}!`;
    ctx.window.showMessage(message);

    return {
      message,
      readme,
      calls,
      workspace: ctx.workspace.root
    };
  });
}

export function deactivate() {
  if (!activeContext) return;
  activeContext.storage.set("deactivated", true);
  activeContext.log("hello-node deactivated");
  activeContext = null;
}
