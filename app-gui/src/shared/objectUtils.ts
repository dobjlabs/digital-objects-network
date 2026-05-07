import type { QualifiedNamePayload } from "./api/wireTypes";

interface ObjectLike {
  status: string;
}

export function isLiveObject(object: ObjectLike): boolean {
  return object.status === "live";
}

export function isNullifiedObject(object: ObjectLike): boolean {
  return object.status === "nullified";
}

/**
 * Canonical printable form `<plugin>::<name>` matching podlang's
 * namespaced-predicate syntax. Use this for tooltips, drag payloads, or
 * anywhere a single-string identifier is needed.
 */
export function qualifiedId(q: QualifiedNamePayload): string {
  return `${q.pluginName}::${q.name}`;
}

/**
 * `true` when two qualified names refer to the same plugin-scoped class or
 * action. The wire type is structural, so equality is field-by-field.
 */
export function qualifiedEq(
  a: QualifiedNamePayload,
  b: QualifiedNamePayload,
): boolean {
  return a.pluginName === b.pluginName && a.name === b.name;
}

/**
 * Label that always includes the originating plugin name so two plugins
 * that expose the same bare name (e.g. `Wood` or `MakeFoo`) stay visually
 * distinguishable in lists, headers, and slot chips. Returns just the bare
 * name when the qualified name has no plugin (shouldn't happen in normal
 * data flows but falls back gracefully).
 */
export function pluginScopedLabel(q: QualifiedNamePayload): string {
  return q.pluginName ? `${q.name} (${q.pluginName})` : q.name;
}

function normalizeObjectsDir(objectsDirPath: string): string {
  return objectsDirPath.replace(/[\\/]+$/, "");
}

function normalizeSlashes(path: string): string {
  return path.replace(/\\/g, "/");
}

function homeAliasForPath(path: string): string | null {
  const normalized = normalizeSlashes(path);
  const unixMatch = normalized.match(/^\/(?:Users|home)\/[^/]+\/(.+)$/);
  if (unixMatch) {
    return `~/${unixMatch[1]}`;
  }
  const windowsMatch = normalized.match(/^[A-Za-z]:\/Users\/[^/]+\/(.+)$/);
  if (windowsMatch) {
    return `~/${windowsMatch[1]}`;
  }
  return null;
}

function pathSeparatorFor(basePath: string): "/" | "\\" {
  if (basePath.includes("\\") && !basePath.includes("/")) {
    return "\\";
  }
  return "/";
}

export function joinObjectsDirPath(
  objectsDirPath: string,
  fileName: string,
  options?: { nullified?: boolean },
): string {
  const base = normalizeObjectsDir(objectsDirPath);
  const sep = pathSeparatorFor(base);
  if (options?.nullified) {
    return [base, ".nullified", fileName].join(sep);
  }
  return [base, fileName].join(sep);
}

export function displayPathInObjectsDir(
  path: string,
  objectsDirPath: string,
): string {
  const normalizedPath = normalizeSlashes(path);
  const normalizedBase = normalizeSlashes(normalizeObjectsDir(objectsDirPath));
  const basePrefix = `${normalizedBase}/`;
  if (normalizedPath !== normalizedBase && !normalizedPath.startsWith(basePrefix)) {
    return path;
  }

  const aliasedBase = homeAliasForPath(normalizedBase);
  if (!aliasedBase) {
    return path;
  }

  const suffix = normalizedPath.slice(normalizedBase.length);
  return `${aliasedBase}${suffix}`;
}
