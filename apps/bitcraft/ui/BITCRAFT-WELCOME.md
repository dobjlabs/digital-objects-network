# Welcome to Bitcraft

Bitcraft is a crafting game where everything you own is a digital object. Each item (a piece of wood, a stick, an axe) is its own `.dobj` file in your objects folder, held privately by you. Every craft is proved and recorded on Ethereum, so nobody can counterfeit or duplicate your items, but only you can see what you hold.

## How to play

1. **Gather your first resource.** Right-click the empty space in the object list and click **bitcraft::GatherWood**.
2. **Watch the job.** It appears under **Active and completed jobs** in the bottom-left. A job takes a little while: your computer proves the action, then waits for it to land on chain. New items show up in the list in amber while pending, and turn black once confirmed.
3. **Craft with what you have.** Select the exact items a recipe needs and right-click the selection. The menu lists every action that takes exactly those items; click one to run it.
4. **Look things up.** Click an item to see its details. The **Recipebook** tab at the top lists every action and what it takes and makes.

Selecting works like Finder: **Cmd-click** adds or removes one item, and **Shift-click** selects a range. Items are sorted by name, so items of the same kind sit together, and a Shift-click range is the quickest way to select many of them.

Only one job runs at a time. Items a running job is using are greyed out until it finishes.

## Your first goals

1. **Gather 2 Wood.** Run **GatherWood** twice.
2. **Make sticks.** Select 1 Wood and run **CraftStick** to get 2 Sticks.
3. **Make an axe.** Select 1 Wood and 2 Sticks and run **CraftWoodAxe**.
4. **Chop.** Select the axe and run **UseWoodAxe**. Each use gives 2 Wood, much more than gathering by hand.

## Gathering

Gathering makes a resource from nothing, but your computer has to do some work first.

| Action | Makes | Effort |
| --- | --- | --- |
| GatherWood | 1 Wood | low |
| GatherStone | 1 Stone | medium |
| GatherIron | 1 Iron | high |

## Crafting

| Action | Uses | Makes |
| --- | --- | --- |
| CraftStick | 1 Wood | 2 Sticks |
| CraftWoodAxe | 1 Wood, 2 Sticks | Wood Axe |
| CraftStoneAxe | Wood Axe, 1 Stone | Stone Axe |
| CraftIronAxe | Stone Axe, 1 Iron | Iron Axe |
| BuildQuarry | 20 Wood, 10 Stone | Quarry (level 1) |
| BuildMiningDrill | 10 Wood, 50 Stone, 5 Iron | Mining Drill (level 1) |
| CraftBucketOfWood, CraftBucketOfStone, CraftBucketOfIron | 1 Wood | an empty bucket |
| BitsFromWood, BitsFromStone, BitsFromIron | 1 Wood, Stone or Iron | 1, 2 or 5 Bits |

## Tools

**Axes** make Wood. A new axe starts with 100 durability, and each use takes 1 away; at 0 it can no longer be used. Upgrading an axe uses up the old one and keeps its remaining durability.

| Action | Makes per use |
| --- | --- |
| UseWoodAxe | 2 Wood |
| UseStoneAxe | 3 Wood |
| UseIronAxe | 4 Wood |

**Quarries** make Stone and **Mining Drills** make Iron. They never wear out, and they make more the higher their level (up to level 6). Run the use action that matches the tool's level; the right-click menu only offers that one.

| Action | Uses | Makes per use |
| --- | --- | --- |
| UseQuarry1 .. UseQuarry6 | nothing | level + 1 Stone |
| UseMiningDrill1 .. UseMiningDrill6 | 5 Wood | level + 1 Iron |

## Buckets and leveling up

A bucket holds any number of one resource in a single item, which keeps your list short. Fill it with **BucketOfWoodAdd1**, **Add5**, **Add10** or **Add50** (selecting the bucket and that many Wood), and take one back out with **BucketOfWoodTake1**. Stone and Iron buckets work the same way.

Leveling up a Quarry or Mining Drill takes full buckets: each bucket must hold **exactly** the amount listed, and the buckets are used up.

| Tool level | Quarry needs | Mining Drill needs |
| --- | --- | --- |
| 1 to 2 | Wood 40, Stone 20 | Wood 20, Stone 100, Iron 10 |
| 2 to 3 | Wood 80, Stone 40 | Wood 40, Stone 200, Iron 20 |
| 3 to 4 | Wood 160, Stone 80 | Wood 80, Stone 400, Iron 40 |
| 4 to 5 | Wood 320, Stone 160 | Wood 160, Stone 800, Iron 80 |
| 5 to 6 | Wood 640, Stone 320 | Wood 320, Stone 1600, Iron 160 |

Select the tool and its buckets together, then right-click to find the matching **LevelUp** action.

## Bits

Turning a resource into Bits takes a short, fixed amount of computing time. No recipe uses Bits yet.

Have fun crafting!
