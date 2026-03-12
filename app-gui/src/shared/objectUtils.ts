interface ObjectLike {
  nullifier: string | null;
}

export function isLiveObject(object: ObjectLike): boolean {
  return object.nullifier == null;
}

export function displayObjectFileName(className: string): string {
  return `${className}.dobj`;
}

function normalizeObjectsDir(objectsDirPath: string): string {
  return objectsDirPath.replace(/[\\/]+$/, "");
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
