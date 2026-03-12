import type { InventoryObjectPayload as InventoryObject } from "./api/wireTypes";

export const objectDisplayFileName = (
  item: Pick<InventoryObject, "className">,
) => `${item.className}.dobj`;

export const objectDisplayFileNameForClass = (className: string) =>
  `${className}.dobj`;
