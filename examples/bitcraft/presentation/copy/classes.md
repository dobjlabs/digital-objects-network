# Bitcraft classes

The summary and description apps show for each bitcraft class. Each `##`
heading is a class name; the first paragraph under it is the one-line
summary, and the paragraphs after it are the description. Run `just
bitcraft-presentation` after editing. See `../README.md`.

## Wood

A basic building material.

Requires proof-of-work to produce, as well as a pocketknife or an axe. The cheapest resource--fast to produce, and used as a basic building block to craft various tools and upgrades. Can be converted directly to bits at a rate of 1 bit per wood.

## Stone

A basic building material.

Requires proof-of-work to produce, as well as a pocketknife or a quarry. An easy-to-produce resource used as a building block for crafting various tools and upgrades. Can be converted directly to bits at a rate of 2 bits per stone.

## Iron

A basic building material.

Requires proof-of-work to produce, as well as a pocketknife or a mining drill. Slightly more expensive to produce than wood or stone, and used as an ingredient into higher-level tools and upgrades. Can be converted directly to bits at a rate of 5 bits per iron.

## Stick

A stick.

Produced from wood. Used for crafting axes.

## BucketOfWood

Holds a stack of wood.

Holds any amount of wood. You can add 1, 5, 10, or 50 wood at a time to the bucket. You can remove wood from the bucket so long as the bucket is not empty.

## BucketOfStone

Holds a stack of stone.

Holds any amount of stone. You can add 1, 5, 10, or 50 stone at a time to the bucket. You can remove stone from the bucket so long as the bucket is not empty.

## BucketOfIron

Holds a stack of iron.

Holds any amount of iron. You can add 1, 5, 10, or 50 iron at a time to the bucket. You can remove iron from the bucket so long as the bucket is not empty.

## Pocketknife

A basic tool for gathering wood, stone, and iron.

A basic tool that allows you to gather wood, stone, and iron. Gathers resources very slowly. Infinite durability.

## WoodAxe

A basic axe for chopping wood.

Chops 2 wood per use, and breaks after 100 uses. Can be upgraded to a stone axe.

## StoneAxe

A solid axe for chopping wood.

Chops 3 wood per use, and breaks after 100 uses. Made by upgrading a wood axe with 1 stone. Can be upgraded to an iron axe.

## IronAxe

A powerful axe for chopping wood.

Chops 4 wood per use, and breaks after 100 uses. Made by upgrading a stone axe with 1 iron.

## Quarry

Mines stone in bulk.

Requires wood and stone to build or upgrade: 20 wood and 10 stone for a level 1 quarry, with upgrade costs doubling at every level. Mines 2 stone per action at level 1, 3 stone per action at level 2, etc.

## MiningDrill

Mines iron in bulk.

Requires wood, stone, and iron to build or upgrade: 10 wood, 50 stone, and 5 iron for a level 1 mining drill, with upgrade costs doubling at every level. Mines 2 iron per action at level 1, 3 iron per action at level 2, etc. Consumes 5 wood per mining action.

## Bit

The currency of Bitcraft.

Can be produced from wood, stone, or iron at a rate of 1 bit per wood, 2 bits per stone, or 5 bits per iron.
