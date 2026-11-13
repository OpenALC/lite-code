export function activate(ctx) {
  ctx.commands.register('demo.webview.ping', (args) => {
    return { pong: true, from: args?.from ?? null };
  });
  ctx.log('demo.webview-sandbox activated');
}
export function deactivate() {}
