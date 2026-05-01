interface ObjectLike {
  status: string;
}

export function isLiveObject(object: ObjectLike): boolean {
  return object.status === "live";
}

export function isNullifiedObject(object: ObjectLike): boolean {
  return object.status === "nullified";
}

export function displayObjectFileName(className: string): string {
  return `${className}.dobj`;
}

/**
 * Render a class label that includes a plugin disambiguator only when the
 * bare class name collides with another loaded plugin. Keeps the common
 * single-plugin case clean.
 */
export function classDisplayLabel(
  displayName: string,
  pluginName: string,
  nameCollisions: string[],
): string {
  if (!pluginName || !nameCollisions.includes(displayName)) {
    return displayName;
  }
  return `${displayName} (${pluginName})`;
}

/**
 * Render an action label with the same collision rule.
 */
export function actionDisplayLabel(
  displayName: string,
  pluginName: string,
  nameCollisions: string[],
): string {
  return classDisplayLabel(displayName, pluginName, nameCollisions);
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
