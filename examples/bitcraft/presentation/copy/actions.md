# Bitcraft actions

The summary and description apps show for each bitcraft action. Each `##`
heading is an action name, or a family of actions like `UseQuarry{level}`
whose text fills in `{level}`; an entry for one action overrides its
family. The first paragraph under a heading is the one-line summary, and
the paragraphs after it are the description. Run `just
bitcraft-presentation` after editing. See `../README.md` for placeholders,
filters and arithmetic.

## CraftPocketknife

Create a pocketknife. Requires no inputs.

Create a pocketknife, an infinite-durability tool used for gathering wood, stone, and iron.

## GatherWood

Use a pocketknife to produce 1 wood. (~15 seconds)

Produce 1 wood using a pocketknife. Requires performing a small amount of proof-of-work hash mining, approximately ~15 seconds on a modern CPU.

## GatherStone

Use a pocketknife to produce 1 stone. (~30 seconds)

Produce 1 stone using a pocketknife. Requires performing a small amount of proof-of-work hash mining, approximately ~30 seconds on a modern CPU.

## GatherIron

Use a pocketknife to produce 1 iron. (~60 seconds)

Produce 1 iron using a pocketknife. Requires performing a small amount of proof-of-work hash mining, approximately ~60 seconds on a modern CPU.

## CraftBucketOf{resource}

Make an empty bucket for {resource|lower} from 1 iron.

A bucket holds a stack of {resource|lower}. You can add and remove {resource|lower} to it via the appropriate actions.

## BucketOf{resource}Add{amount}

Put {amount} {resource|lower} into a bucket.

Spends {amount} {resource|lower} and adds them to the bucket's count.

## BucketOf{resource}Take{amount}

Take {amount} {resource|lower} out of a bucket.

Lowers the bucket's count by {amount} and gives you {amount}
{resource|lower}. The bucket must hold at least that many.

## CraftBarrelOfOil

TODO

TODO

## CraftStick

Craft 2 sticks from 1 wood.

Consume 1 wood to produce 2 sticks.

## CraftWoodAxe

Make a wooden axe from 2 sticks and 1 wood.

Consumes 2 sticks and 1 wood to create a wooden axe, which chops wood twice as fast as a pocketknife. Axes start with 100 durability, meaning that they are unusable after 100 uses.

## CraftStoneAxe

Upgrade a wooden axe to a stone axe using 1 stone.

Consumes a wooden axe and 1 stone to create a stone axe, which chops wood three times as fast as a pocketknife. Axes start with 100 durability, meaning that they are unusable after 100 uses.

## CraftIronAxe

Upgrade a stone axe to an iron axe using 1 iron.

Consumes a stone axe and 1 iron to create an iron axe, which chops wood four times as fast as a pocketknife. Axes start with 100 durability, meaning that they are unusable after 100 uses.

## UseWoodAxe

Use a wooden axe to produce 2 wood. (~15 seconds)

Produce 2 wood using a wooden axe. Requires performing a small amount of proof-of-work hash mining, approximately ~15 seconds on a modern CPU. Consumes 1 durability of the axe.

## UseStoneAxe

Use a stone axe to produce 3 wood. (~15 seconds)

Produce 3 wood using a stone axe. Requires performing a small amount of proof-of-work hash mining, approximately ~15 seconds on a modern CPU. Consumes 1 durability of the axe.

## UseIronAxe

Use an iron axe to produce 4 wood. (~15 seconds)

Produce 4 wood using an iron axe. Requires performing a small amount of proof-of-work hash mining, approximately ~15 seconds on a modern CPU. Consumes 1 durability of the axe.

## UseQuarry{level}

Use a level {level} quarry to produce {level + 1} stone.

Produce {level + 1} stone using a quarry. Requires performing a small amount of proof-of-work hash mining, approximately ~30 seconds on a modern CPU.

## UseMiningDrill{level}

Use a level {level} mining drill to produce {level + 1} iron.

Produce {level + 1} iron using a mining drill. Consumes 5 wood, and requires performing a small amount of proof-of-work hash mining, approximately ~60 seconds on a modern CPU.

## MineCoal{level}

TODO

TODO

## MineGem{level}

TODO

TODO

## CrackGeode{gem}

TODO

TODO

## PumpOil

TODO

TODO

## BuildQuarry

Make a level 1 quarry using 20 wood and 10 stone.

A level 1 quarry mines stone twice as fast as a pocketknife. Quarries have infinite durability, and can be upgraded to produce stone even faster.

## BuildMiningDrill

Make a level 1 mining drill using 10 wood, 50 stone, and 5 iron.

A level 1 mining drill mines iron twice as fast as a pocketknife. Mining drills have infinite durability, and can be upgraded to produce iron even faster.

## BuildPumpjack

TODO

TODO

## LevelUpQuarry{level}

Upgrades a level {level} quarry to level {level + 1}.

Only works on a quarry at exactly level {level}. Spends a bucket holding
exactly {20 * 2 ** level} wood and a bucket holding exactly {10 * 2 **
level} stone. A level {level + 1} quarry mines stone {level + 2} times as fast as a pocketknife.

## LevelUpMiningDrill{level}

Upgrades a level {level} mining drill to level {level + 1}.

Only works on a mining drill at exactly level {level}. Spends buckets
holding exactly {10 * 2 ** level} wood, {50 * 2 ** level} stone and {5 * 2
** level} iron. A level {level + 1} mining drill mines iron {level + 2} times as fast as a pocketknife.

## LevelUpPumpjack{level}

TODO

TODO

## BitsFromWood

Generate bits from wood.

Converts 1 wood into 1 bit.

## BitsFromStone

Generate bits from stone.

Converts 1 stone into 2 bits.

## BitsFromIron

Generate bits from iron.

Converts 1 iron into 5 bits.

## BitsFrom{gem}

TODO

TODO

## Rekey{class}

Refreshes this file, invalidating all other copies on the network.

By rekeying this item, all copies of the file on other machines (except for this one) will be marked as stale and unusable.
