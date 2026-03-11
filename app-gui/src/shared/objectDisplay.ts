import type { InventoryItem } from "./types/domain";

export const objectDisplayFileName = (
  item: Pick<InventoryItem, "classMeta">,
) => `${item.classMeta.name}.dobj`;

export const objectDisplayFileNameForClass = (className: string) =>
  `${className}.dobj`;
