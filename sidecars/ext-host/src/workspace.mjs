import fsPromises from "node:fs/promises";
import path from "node:path";

export function isPathInside(root, target) {
  const relative = path.relative(root, target);
  return (
    relative === "" ||
    (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative))
  );
}

export function createWorkspace(root) {
  if (typeof root !== "string" || root.trim() === "") {
    throw new TypeError("workspaceRoot must be a non-empty string");
  }
  const workspaceRoot = path.resolve(root);

  return Object.freeze({
    root: workspaceRoot,
    async readFile(relativePath) {
      if (typeof relativePath !== "string" || relativePath.length === 0) {
        throw new TypeError("workspace.readFile expects a non-empty path string");
      }
      if (relativePath.includes("\0")) {
        throw new TypeError("workspace.readFile path contains a null byte");
      }
      const target = path.resolve(workspaceRoot, relativePath);
      if (!isPathInside(workspaceRoot, target)) {
        throw new Error(`Path is outside the workspace: ${relativePath}`);
      }
      return fsPromises.readFile(target, "utf8");
    }
  });
}
