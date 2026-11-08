import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const STORAGE_FILE = ".litecode-storage.json";
const FALLBACK_DIR = path.join(".lite-code", "plugin-storage");

export function storagePaths(pluginId, pluginDir) {
  const safeId =
    String(pluginId)
      .replace(/[^A-Za-z0-9._-]/g, "_")
      .slice(0, 120) || "plugin";
  return {
    primary: path.join(pluginDir, STORAGE_FILE),
    fallback: path.join(os.homedir(), FALLBACK_DIR, `${safeId}.json`)
  };
}

export function readStorageObject(file) {
  let text;
  try {
    text = fs.readFileSync(file, "utf8");
  } catch (error) {
    return error.code === "ENOENT"
      ? { status: "missing" }
      : { status: "unreadable", error };
  }

  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    return { status: "invalid", reason: error.message };
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { status: "invalid", reason: "expected a JSON object" };
  }

  const data = Object.create(null);
  for (const key of Object.keys(parsed)) data[key] = parsed[key];
  return { status: "ok", data };
}

export function createPluginStorage(pluginId, pluginDir, log) {
  const files = storagePaths(pluginId, pluginDir);
  let activeFile = files.primary;
  let data = Object.create(null);
  let lastPersistError = "";

  function reportPersistError(message) {
    if (message !== lastPersistError) {
      lastPersistError = message;
      log(message);
    }
  }

  const primary = readStorageObject(files.primary);
  if (primary.status === "ok") {
    data = primary.data;
  } else if (primary.status === "invalid") {
    log(`ignoring invalid JSON in ${files.primary}: ${primary.reason}`);
  } else {
    if (primary.status === "unreadable") {
      log(`cannot read plugin storage ${files.primary}: ${primary.error.message}`);
    }
    const fallback = readStorageObject(files.fallback);
    if (fallback.status === "ok") {
      data = fallback.data;
      activeFile = files.fallback;
    } else if (fallback.status === "invalid") {
      log(`ignoring invalid JSON in ${files.fallback}: ${fallback.reason}`);
    } else if (fallback.status === "unreadable") {
      log(`cannot read plugin storage ${files.fallback}: ${fallback.error.message}`);
    }
  }

  function writeStorage(file, text) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text, { encoding: "utf8", mode: 0o600 });
  }

  function persist() {
    let text;
    try {
      text = `${JSON.stringify(data, null, 2)}\n`;
    } catch (error) {
      reportPersistError(`cannot serialize plugin storage for ${pluginId}: ${error.message}`);
      return;
    }

    try {
      writeStorage(activeFile, text);
      lastPersistError = "";
      return;
    } catch (error) {
      reportPersistError(`cannot persist plugin storage to ${activeFile}: ${error.message}`);
    }

    if (activeFile !== files.fallback) {
      try {
        writeStorage(files.fallback, text);
        activeFile = files.fallback;
        lastPersistError = "";
        log(`using fallback plugin storage at ${files.fallback}`);
      } catch (error) {
        reportPersistError(`cannot persist plugin storage to ${files.fallback}: ${error.message}`);
      }
    }
  }

  function checkKey(key) {
    if (typeof key !== "string" || key.length === 0) {
      throw new TypeError("storage key must be a non-empty string");
    }
  }

  return Object.freeze({
    get(key) {
      checkKey(key);
      return Object.hasOwn(data, key) ? data[key] : undefined;
    },
    set(key, value) {
      checkKey(key);
      let encoded;
      try {
        encoded = JSON.stringify(value);
      } catch (error) {
        throw new TypeError(`storage value for "${key}" is not JSON-serializable: ${error.message}`);
      }
      if (encoded === undefined) {
        throw new TypeError(`storage value for "${key}" is not JSON-serializable`);
      }
      data[key] = JSON.parse(encoded);
      persist();
    }
  });
}
