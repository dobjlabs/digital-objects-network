export function truncateDisplayHash(value: string): string {
  const trimmed = value.trim();
  if (!/^0x[0-9a-f]+$/i.test(trimmed)) return trimmed;
  if (trimmed.length <= 14) return trimmed;
  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`;
}
