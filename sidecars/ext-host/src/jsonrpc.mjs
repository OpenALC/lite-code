const JSONRPC_VERSION = "2.0";

export const RPC_PARSE_ERROR = -32700;
export const RPC_INVALID_REQUEST = -32600;
export const RPC_METHOD_NOT_FOUND = -32601;
export const RPC_INVALID_PARAMS = -32602;
export const RPC_INTERNAL_ERROR = -32000;

export function resultResponse(id, result) {
  return { jsonrpc: JSONRPC_VERSION, id, result };
}

export function errorResponse(id, code, message, data) {
  const response = {
    jsonrpc: JSONRPC_VERSION,
    id: id === undefined ? null : id,
    error: { code, message }
  };
  if (data !== undefined) response.error.data = data;
  return response;
}

export function thrownDetails(error) {
  if (error instanceof Error) {
    return {
      name: error.name || "Error",
      message: error.message || String(error),
      stack: typeof error.stack === "string" ? error.stack : undefined
    };
  }
  return { name: "Error", message: String(error), stack: undefined };
}

export function serializableResult(value) {
  let text;
  try {
    text = JSON.stringify(value);
  } catch (error) {
    throw new Error(`result is not JSON-serializable: ${thrownDetails(error).message}`);
  }
  if (text === undefined) return null;
  return JSON.parse(text);
}
