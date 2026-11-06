// sidecar 的 JSON-RPC 错误码：db_unavailable / db_rejected / db_read_only /
// invalid_argument。旧版 Rust 侧按这些字符串映射契约错误码。

export class BridgeError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'BridgeError';
    this.code = code;
  }
}

export const invalidArgument = (message) => new BridgeError('invalid_argument', message);
export const dbRejected = (message) => new BridgeError('db_rejected', message);
export const dbReadOnly = (message) => new BridgeError('db_read_only', message);
export const dbUnavailable = (message) => new BridgeError('db_unavailable', message);

export function messageOf(error) {
  if (error instanceof Error) return error.message;
  return String(error);
}

export function sanitize(message, secret) {
  if (typeof secret !== 'string' || secret.length === 0) return message;
  return message.split(secret).join('***');
}
