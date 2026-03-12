import type { InventoryObject } from "./types/domain";

export const objectDisplayFileName = (
  item: Pick<InventoryObject, "className">,
) => `${item.className}.dobj`;

export const objectDisplayFileNameForClass = (className: string) =>
  `${className}.dobj`;
