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

Craft an empty barrel for holding oil, from 5 iron.

A barrel holds a variable quantity of oil. You can pump oil into the barrel using a pumpjack.

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

## MineIron{level}

Use a level {level} mining drill to produce {level + 1} iron.

Produce {level + 1} iron using a mining drill. Consumes 5 wood, and requires performing a small amount of proof-of-work hash mining, approximately ~60 seconds on a modern CPU.

## MineCoal{level}

Use a level {level} mining drill to produce {level} coal.

Produce {level} coal using a mining drill. Consumes 5 wood, and requires performing a small amount of proof-of-work hash mining, approximately ~60 seconds on a modern CPU.

## MineGeode{level}

Use a level {level} mining drill to mine a Geode.

Produce a geode using a mining drill. Consumes 5 wood, and requires performing a moderate amount of proof-of-work hash mining. Higher level mining drills find geodes faster: at level N, finding a geode takes about 20 / N minutes on average. Once found, a geode can be cracked open to reveal either a quartz, an amethyst, a sapphire, a emerald, a ruby, or a diamond.

## CrackGeode{gem}

Extracts a gem ({gem}) from your Geode.

Cracks open this Geode, revealing {gem}.

## PumpOil

Use a pumpjack to pump oil into an oil barrel. Consumes 5 coal.

Produce a variable amount of oil using a pumpjack. Requires 5 coal and a barrel for the oil to be pumped into. You can specify the number of steps you'd like to operate the pumpjack for: each step requires performing serial (non-parallelizable) cryptographic work (about 5 minutes per step on a typical laptop), though the 5 coal input cost is fixed. Higher level pumpjacks produce more oil per step.

## BuildQuarry

Build a level 1 quarry using 20 wood and 10 stone.

A level 1 quarry mines stone twice as fast as a pocketknife. Quarries have infinite durability, and can be upgraded to produce stone even faster.

## BuildMiningDrill

Build a level 1 mining drill using 10 wood, 50 stone, and 5 iron.

A level 1 mining drill mines iron twice as fast as a pocketknife. Mining drills have infinite durability, and can be upgraded to produce iron even faster.

## BuildPumpjack

Build a level 1 pumpjack using a bucket of 50 stone, a bucket of 100 iron, and 1 quartz.

A pumpjack allows you to produce oil. Pumping oil requires a pumpjack, a barrel for holding the oil, and 5 coal.

## LevelUpQuarry{level}

Upgrades a level {level} quarry to level {level + 1}.

Only works on a quarry at exactly level {level}. Spends a bucket holding
at least {20 * 2 ** level} wood and a bucket holding at least {10 * 2 **
level} stone. A level {level + 1} quarry mines stone {level + 2} times as fast as a pocketknife.

## LevelUpMiningDrill{level}

Upgrades a level {level} mining drill to level {level + 1}.

Only works on a mining drill at exactly level {level}. Spends buckets
holding at least {10 * 2 ** level} wood, {50 * 2 ** level} stone, and {5 * 2
** level} iron. A level {level + 1} mining drill mines iron {level + 2} times as fast as a pocketknife. Higher level mining drills also mine more coal per use, and find geodes faster.

## LevelUpPumpjack{level}

Upgrade a level {level} pumpjack to level {level + 1}.

Only works on a pumpjack at exactly level {level}. Spends a bucket holding at least {50 * 2 ** level} stone, a bucket holding at least {100 * 2 ** level} iron, and a bucket holding at least {2 ** level} quartz. Higher level pumpjacks produce more oil per step they are run.

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

Generate bits from a gem. Requires a bucket of bits to put the bits into.

Converts a gem into bits, which are put into a bucket. A quartz is converted into 100 bits; amethyst into 2,000 bits; sapphire into 40,000 bits; emerald into 800,000 bits; ruby into 16,000,000 bits; and diamond into 320,000,000 bits.

## Rekey{class}

Refreshes this file, invalidating all other copies on the network.

By rekeying this item, all copies of the file on other machines (except for this one) will be marked as stale and unusable.
