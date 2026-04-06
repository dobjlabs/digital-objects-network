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
