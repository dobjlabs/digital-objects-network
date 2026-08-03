function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isPod2IntWrapper(value: unknown): value is { Int: string | number } {
  return isRecord(value) && Object.keys(value).length === 1 && "Int" in value;
}

function parsePod2IntWrapper(value: unknown): number | null {
  if (!isPod2IntWrapper(value)) return null;
  const raw = value.Int;
  const parsed =
    typeof raw === "number" ? raw : Number.parseInt(String(raw), 10);
  return Number.isFinite(parsed) ? parsed : null;
}

function normalizePod2ContainerInner(value: unknown): unknown {
  if (!Array.isArray(value)) {
    return value;
  }

  const tupleEntries = value.every(
    (entry) => Array.isArray(entry) && entry.length >= 1 && entry.length <= 2,
  )
    ? (value as unknown[][])
    : null;

  if (!tupleEntries) {
    return value.map((entry) => normalizePod2Value(entry));
  }

  if (tupleEntries.every((entry) => entry.length === 1)) {
    return tupleEntries.map(([entry]) => normalizePod2Value(entry));
  }

  if (
    tupleEntries.every(
      (entry) => entry.length === 2 && typeof entry[0] === "string",
    )
  ) {
    return Object.fromEntries(
      tupleEntries.map(([key, entry]) => [
        key as string,
        normalizePod2Value(entry),
      ]),
    );
  }

  if (
    tupleEntries.every(
      (entry) => entry.length === 2 && parsePod2IntWrapper(entry[0]) !== null,
    )
  ) {
    return tupleEntries
      .map(
        ([index, entry]) =>
          [parsePod2IntWrapper(index) ?? 0, normalizePod2Value(entry)] as const,
      )
      .sort((left, right) => left[0] - right[0])
      .map(([, entry]) => entry);
  }

  return tupleEntries.map((entry) =>
    entry.map((item) => normalizePod2Value(item)),
  );
}

const CONTAINER_KIND_PATTERN = /^[d-][s-][a-]$/;

function isPod2Container(
  value: Record<string, unknown>,
): value is { kind: string; kvs: unknown[] } {
  return (
    Object.keys(value).length === 2 &&
    typeof value.kind === "string" &&
    CONTAINER_KIND_PATTERN.test(value.kind) &&
    Array.isArray(value.kvs)
  );
}

/**
 * pod2 containers serialize as `{ kind, kvs }`: `kind` is a three-flag
 * string ("d--" dictionary, "-s-" set, "--a" array; flags can combine when
 * one root has been used as several types) and `kvs` holds `[key, value]`
 * entries, collapsed to `[value]` when the key equals the value (always the
 * case for sets). Unwrap by the claimed kind; a kvs whose entries do not
 * match it falls back to the shape heuristics of
 * `normalizePod2ContainerInner`.
 */
function normalizePod2Container(kind: string, kvs: unknown[]): unknown {
  const entryPair = (entry: unknown): [unknown, unknown] | null => {
    if (!Array.isArray(entry) || entry.length < 1 || entry.length > 2) {
      return null;
    }
    return [entry[0], entry[entry.length - 1]];
  };
  const pairs = kvs.map(entryPair);

  if (kind[0] === "d") {
    if (pairs.every((pair) => pair !== null && typeof pair[0] === "string")) {
      return Object.fromEntries(
        (pairs as [string, unknown][]).map(([key, entry]) => [
          key,
          normalizePod2Value(entry),
        ]),
      );
    }
  } else if (kind[1] === "s") {
    if (pairs.every((pair) => pair !== null)) {
      return (pairs as [unknown, unknown][]).map(([, entry]) =>
        normalizePod2Value(entry),
      );
    }
  } else if (kind[2] === "a") {
    const slots = pairs.map((pair) => {
      if (!pair) return null;
      const index = parsePod2IntWrapper(pair[0]);
      if (index === null) return null;
      return [index, normalizePod2Value(pair[1])] as const;
    });
    if (slots.every((slot) => slot !== null)) {
      return (slots as (readonly [number, unknown])[])
        .sort((left, right) => left[0] - right[0])
        .map(([, entry]) => entry);
    }
  }
  return normalizePod2ContainerInner(kvs);
}

function normalizePod2Value(value: unknown): unknown {
  if (typeof value === "string") {
    return value
      .trim()
      .replace(/^Raw\((.*)\)$/, "$1")
      .trim();
  }

  if (
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "bigint" ||
    value == null
  ) {
    return value;
  }

  if (Array.isArray(value)) {
    return value.map((entry) => normalizePod2Value(entry));
  }

  if (!isRecord(value)) {
    return value;
  }

  if (isPod2Container(value)) {
    return normalizePod2Container(value.kind, value.kvs);
  }

  const keys = Object.keys(value);
  if (keys.length === 1) {
    const key = keys[0];
    const inner = value[key];

    if (
      key === "Raw" ||
      key === "Int" ||
      key === "PublicKey" ||
      key === "SecretKey" ||
      key === "Predicate" ||
      key === "Root" ||
      key === "set" ||
      key === "array"
    ) {
      return normalizePod2Value(inner);
    }

    if (key === "kvs" && isRecord(inner)) {
      return Object.fromEntries(
        Object.entries(inner).map(([entryKey, entryValue]) => [
          entryKey,
          normalizePod2Value(entryValue),
        ]),
      );
    }

    // `_raw` is the daemon's fallback wrapper for an object dictionary that
    // serialized as a pod2 container sequence ([key, value] entries) rather
    // than a plain object; unwrap it the same way as `inner`.
    if (key === "inner" || key === "_raw") {
      return normalizePod2ContainerInner(inner);
    }
  }

  return Object.fromEntries(
    Object.entries(value).map(([key, entry]) => [
      key,
      normalizePod2Value(entry),
    ]),
  );
}

export { isRecord, normalizePod2Value };
