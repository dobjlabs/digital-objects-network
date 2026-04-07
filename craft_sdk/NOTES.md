# Crafting predicates

```
FindLog(log, tx, tx0, private: log0, work) = AND (
  // Output
  DictContains(log0, "blueprint", "Log")
  Vdf(3, log0, work)
  DictUpdate(log, log0, "work", work)
  tx::TxInserted(tx, tx0, log)
)

CraftWood(wood, tx, tx0, private: tx1, log, wood0, key) = AND (
  // Input
  IsLog(log)
  tx::TxDeleted(tx1, tx0, log)
  // Output
  DictContains(wood0, "blueprint", "Wood")
  DictUpdate(wood, wood0, "key", key)
  LtEqU256(wood, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))
  tx::TxInserted(tx, tx1, wood)
)

CraftSticks(stick_a, stick_b, tx, tx0, private: tx1, tx2, wood) = AND (
  // Input
  IsWood(wood)
  tx::TxDeleted(tx1, tx0, wood)
  // Output
  DictContains(stick_a, "blueprint", "Stick")
  tx::TxInserted(tx2, tx1, stick_a)
  // Output
  DictContains(stick_b, "blueprint", "Stick")
  tx::TxInserted(tx, tx2, stick_b)
)

CraftWoodPick(wood_pick, tx, tx0, private: tx1, tx2, wood, stick) = AND (
  // Input
  IsWood(wood)
  tx::TxDeleted(tx1, tx0, wood)
  // Input
  IsStick(stick)
  tx::TxDeleted(tx2, tx1, stick)
  // Output
  DictContains(wood_pick, "blueprint", "WoodPick")
  DictContains(wood_pick, "durability", 100)
  tx::TxInserted(tx, tx2, wood_pick)
)

CraftStonePick(stone_pick, tx, tx0, private: tx1, tx2, stone, stick) = AND (
  // Input
  IsStone(stone)
  tx::TxDeleted(tx1, tx0, stone)
  // Input
  IsStick(stick)
  tx::TxDeleted(tx2, tx1, stick)
  // Output
  DictContains(stone_pick, "blueprint", "StonePick")
  DictContains(stone_pick, "durability", 200)
  tx::TxInserted(tx, tx2, stone_pick)
)

UseWoodPick(wood_pick, tx, tx0, private: wood_pick0, wood_pick1, wood_pick2, durability, key, work) = AND (
  // Mutate
  IsWoodPick(wood_pick0)
  Gt(wood_pick0.durability, 0)
  SumOf(wood_pick0.durability, durability, 1)
  DictUpdate(wood_pick1, wood_pick0, "durability", durability)
  DictUpdate(wood_pick2, wood_pick1, "key", key)
  Vdf(10, wood_pick2, work)
  DictUpdate(wood_pick, wood_pick2, "work", work)
  tx::TxMutated(tx, tx0, wood_pick, wood_pick0)
)

MineStoneWithWoodPick(stone, tx, tx0, private: tx1, pick) = AND (
  // Action dependency
  UseWoodPick(pick, tx1, tx0)
  // Output
  DictContains(stone, "blueprint", "Stone")
  tx::TxInserted(tx, tx1, stone)
)

UseStonePick(stone_pick, tx, tx0, private: stone_pick0, stone_pick1, stone_pick2, durability, key, work) = AND (
  // Mutate
  IsStonePick(stone_pick0)
  Gt(stone_pick0.durability, 0)
  SumOf(stone_pick0.durability, durability, 1)
  DictUpdate(stone_pick1, stone_pick0, "durability", durability)
  DictUpdate(stone_pick2, stone_pick1, "key", key)
  Vdf(5, stone_pick2, work)
  DictUpdate(stone_pick, stone_pick2, "work", work)
  tx::TxMutated(tx, tx0, stone_pick, stone_pick0)
)

MineStoneWithStonePick(stone, tx, tx0, private: tx1, pick) = AND (
  // Action dependency
  UseStonePick(pick, tx1, tx0)
  // Output
  DictContains(stone, "blueprint", "Stone")
  tx::TxInserted(tx, tx1, stone)
)

// Classes

IsLog(state, private: tx, tx0) = OR(
  FindLog(state, tx, tx0)
)

IsWood(state, private: tx, tx0) = OR(
  CraftWood(state, tx, tx0)
)

IsStick(state, private: tx, tx0, _other_0) = OR(
  CraftSticks(state, _other_0, tx, tx0)
  CraftSticks(_other_0, state, tx, tx0)
)

IsWoodPick(state, private: tx, tx0) = OR(
  CraftWoodPick(state, tx, tx0)
  UseWoodPick(state, tx, tx0)
)

IsStonePick(state, private: tx, tx0) = OR(
  CraftStonePick(state, tx, tx0)
  UseStonePick(state, tx, tx0)
)

IsStone(state, private: tx, tx0) = OR(
  MineStoneWithWoodPick(state, tx, tx0)
  MineStoneWithStonePick(state, tx, tx0)
)
```
