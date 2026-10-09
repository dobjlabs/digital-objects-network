fn GatherWood(action) {
  var wood000 = action.output("Wood");
  let target = action.top_limb_u256(8339694957348);
  var grind_key = action.pow_obj_grind(wood000, target);
  wood000.update("key", grind_key);
  action.intro_lt_eq_u256(wood000, target);
}

fn GatherStone(action) {
  var stone000 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone000, target);
  stone000.update("key", grind_key);
  action.intro_lt_eq_u256(stone000, target);
}

fn GatherIron(action) {
  var iron000 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron000, target);
  iron000.update("key", grind_key);
  action.intro_lt_eq_u256(iron000, target);
}

fn CraftBucketOfWood(action) {
  var wood000 = action.input("Wood");
  var woodbucket = action.output("BucketOfWood");
  woodbucket.set([["n", 0]]);
}

fn CraftBucketOfStone(action) {
  var wood000 = action.input("Wood");
  var stonebucket = action.output("BucketOfStone");
  stonebucket.set([["n", 0]]);
}

fn CraftBucketOfIron(action) {
  var wood000 = action.input("Wood");
  var ironbucket = action.output("BucketOfIron");
  ironbucket.set([["n", 0]]);
}

fn BucketOfWoodAdd1(action) {
  var wood000 = action.input("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = action.random();
  woodbucket.update("key", key);
  var n = unsafe { woodbucket.n + 1 };
  action.st_sum(woodbucket.n, 1, n);
  woodbucket.update("n", n);
}

fn BucketOfWoodAdd5(action) {
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = action.random();
  woodbucket.update("key", key);
  var n = unsafe { woodbucket.n + 5 };
  action.st_sum(woodbucket.n, 5, n);
  woodbucket.update("n", n);
}

fn BucketOfWoodAdd10(action) {
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var wood005 = action.input("Wood");
  var wood006 = action.input("Wood");
  var wood007 = action.input("Wood");
  var wood008 = action.input("Wood");
  var wood009 = action.input("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = action.random();
  woodbucket.update("key", key);
  var n = unsafe { woodbucket.n + 10 };
  action.st_sum(woodbucket.n, 10, n);
  woodbucket.update("n", n);
}

fn BucketOfWoodAdd50(action) {
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var wood005 = action.input("Wood");
  var wood006 = action.input("Wood");
  var wood007 = action.input("Wood");
  var wood008 = action.input("Wood");
  var wood009 = action.input("Wood");
  var wood010 = action.input("Wood");
  var wood011 = action.input("Wood");
  var wood012 = action.input("Wood");
  var wood013 = action.input("Wood");
  var wood014 = action.input("Wood");
  var wood015 = action.input("Wood");
  var wood016 = action.input("Wood");
  var wood017 = action.input("Wood");
  var wood018 = action.input("Wood");
  var wood019 = action.input("Wood");
  var wood020 = action.input("Wood");
  var wood021 = action.input("Wood");
  var wood022 = action.input("Wood");
  var wood023 = action.input("Wood");
  var wood024 = action.input("Wood");
  var wood025 = action.input("Wood");
  var wood026 = action.input("Wood");
  var wood027 = action.input("Wood");
  var wood028 = action.input("Wood");
  var wood029 = action.input("Wood");
  var wood030 = action.input("Wood");
  var wood031 = action.input("Wood");
  var wood032 = action.input("Wood");
  var wood033 = action.input("Wood");
  var wood034 = action.input("Wood");
  var wood035 = action.input("Wood");
  var wood036 = action.input("Wood");
  var wood037 = action.input("Wood");
  var wood038 = action.input("Wood");
  var wood039 = action.input("Wood");
  var wood040 = action.input("Wood");
  var wood041 = action.input("Wood");
  var wood042 = action.input("Wood");
  var wood043 = action.input("Wood");
  var wood044 = action.input("Wood");
  var wood045 = action.input("Wood");
  var wood046 = action.input("Wood");
  var wood047 = action.input("Wood");
  var wood048 = action.input("Wood");
  var wood049 = action.input("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = action.random();
  woodbucket.update("key", key);
  var n = unsafe { woodbucket.n + 50 };
  action.st_sum(woodbucket.n, 50, n);
  woodbucket.update("n", n);
}

fn BucketOfWoodTake1(action) {
  var wood000 = action.output("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = action.random();
  woodbucket.update("key", key);
  action.st_gt_eq(woodbucket.n, 1);
  var n = unsafe { woodbucket.n - 1 };
  action.st_sum(n, 1, woodbucket.n);
  woodbucket.update("n", n);
}

fn BucketOfStoneAdd1(action) {
  var stone000 = action.input("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = action.random();
  stonebucket.update("key", key);
  var n = unsafe { stonebucket.n + 1 };
  action.st_sum(stonebucket.n, 1, n);
  stonebucket.update("n", n);
}

fn BucketOfStoneAdd5(action) {
  var stone000 = action.input("Stone");
  var stone001 = action.input("Stone");
  var stone002 = action.input("Stone");
  var stone003 = action.input("Stone");
  var stone004 = action.input("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = action.random();
  stonebucket.update("key", key);
  var n = unsafe { stonebucket.n + 5 };
  action.st_sum(stonebucket.n, 5, n);
  stonebucket.update("n", n);
}

fn BucketOfStoneAdd10(action) {
  var stone000 = action.input("Stone");
  var stone001 = action.input("Stone");
  var stone002 = action.input("Stone");
  var stone003 = action.input("Stone");
  var stone004 = action.input("Stone");
  var stone005 = action.input("Stone");
  var stone006 = action.input("Stone");
  var stone007 = action.input("Stone");
  var stone008 = action.input("Stone");
  var stone009 = action.input("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = action.random();
  stonebucket.update("key", key);
  var n = unsafe { stonebucket.n + 10 };
  action.st_sum(stonebucket.n, 10, n);
  stonebucket.update("n", n);
}

fn BucketOfStoneAdd50(action) {
  var stone000 = action.input("Stone");
  var stone001 = action.input("Stone");
  var stone002 = action.input("Stone");
  var stone003 = action.input("Stone");
  var stone004 = action.input("Stone");
  var stone005 = action.input("Stone");
  var stone006 = action.input("Stone");
  var stone007 = action.input("Stone");
  var stone008 = action.input("Stone");
  var stone009 = action.input("Stone");
  var stone010 = action.input("Stone");
  var stone011 = action.input("Stone");
  var stone012 = action.input("Stone");
  var stone013 = action.input("Stone");
  var stone014 = action.input("Stone");
  var stone015 = action.input("Stone");
  var stone016 = action.input("Stone");
  var stone017 = action.input("Stone");
  var stone018 = action.input("Stone");
  var stone019 = action.input("Stone");
  var stone020 = action.input("Stone");
  var stone021 = action.input("Stone");
  var stone022 = action.input("Stone");
  var stone023 = action.input("Stone");
  var stone024 = action.input("Stone");
  var stone025 = action.input("Stone");
  var stone026 = action.input("Stone");
  var stone027 = action.input("Stone");
  var stone028 = action.input("Stone");
  var stone029 = action.input("Stone");
  var stone030 = action.input("Stone");
  var stone031 = action.input("Stone");
  var stone032 = action.input("Stone");
  var stone033 = action.input("Stone");
  var stone034 = action.input("Stone");
  var stone035 = action.input("Stone");
  var stone036 = action.input("Stone");
  var stone037 = action.input("Stone");
  var stone038 = action.input("Stone");
  var stone039 = action.input("Stone");
  var stone040 = action.input("Stone");
  var stone041 = action.input("Stone");
  var stone042 = action.input("Stone");
  var stone043 = action.input("Stone");
  var stone044 = action.input("Stone");
  var stone045 = action.input("Stone");
  var stone046 = action.input("Stone");
  var stone047 = action.input("Stone");
  var stone048 = action.input("Stone");
  var stone049 = action.input("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = action.random();
  stonebucket.update("key", key);
  var n = unsafe { stonebucket.n + 50 };
  action.st_sum(stonebucket.n, 50, n);
  stonebucket.update("n", n);
}

fn BucketOfStoneTake1(action) {
  var stone000 = action.output("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = action.random();
  stonebucket.update("key", key);
  action.st_gt_eq(stonebucket.n, 1);
  var n = unsafe { stonebucket.n - 1 };
  action.st_sum(n, 1, stonebucket.n);
  stonebucket.update("n", n);
}

fn BucketOfIronAdd1(action) {
  var iron000 = action.input("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = action.random();
  ironbucket.update("key", key);
  var n = unsafe { ironbucket.n + 1 };
  action.st_sum(ironbucket.n, 1, n);
  ironbucket.update("n", n);
}

fn BucketOfIronAdd5(action) {
  var iron000 = action.input("Iron");
  var iron001 = action.input("Iron");
  var iron002 = action.input("Iron");
  var iron003 = action.input("Iron");
  var iron004 = action.input("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = action.random();
  ironbucket.update("key", key);
  var n = unsafe { ironbucket.n + 5 };
  action.st_sum(ironbucket.n, 5, n);
  ironbucket.update("n", n);
}

fn BucketOfIronAdd10(action) {
  var iron000 = action.input("Iron");
  var iron001 = action.input("Iron");
  var iron002 = action.input("Iron");
  var iron003 = action.input("Iron");
  var iron004 = action.input("Iron");
  var iron005 = action.input("Iron");
  var iron006 = action.input("Iron");
  var iron007 = action.input("Iron");
  var iron008 = action.input("Iron");
  var iron009 = action.input("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = action.random();
  ironbucket.update("key", key);
  var n = unsafe { ironbucket.n + 10 };
  action.st_sum(ironbucket.n, 10, n);
  ironbucket.update("n", n);
}

fn BucketOfIronAdd50(action) {
  var iron000 = action.input("Iron");
  var iron001 = action.input("Iron");
  var iron002 = action.input("Iron");
  var iron003 = action.input("Iron");
  var iron004 = action.input("Iron");
  var iron005 = action.input("Iron");
  var iron006 = action.input("Iron");
  var iron007 = action.input("Iron");
  var iron008 = action.input("Iron");
  var iron009 = action.input("Iron");
  var iron010 = action.input("Iron");
  var iron011 = action.input("Iron");
  var iron012 = action.input("Iron");
  var iron013 = action.input("Iron");
  var iron014 = action.input("Iron");
  var iron015 = action.input("Iron");
  var iron016 = action.input("Iron");
  var iron017 = action.input("Iron");
  var iron018 = action.input("Iron");
  var iron019 = action.input("Iron");
  var iron020 = action.input("Iron");
  var iron021 = action.input("Iron");
  var iron022 = action.input("Iron");
  var iron023 = action.input("Iron");
  var iron024 = action.input("Iron");
  var iron025 = action.input("Iron");
  var iron026 = action.input("Iron");
  var iron027 = action.input("Iron");
  var iron028 = action.input("Iron");
  var iron029 = action.input("Iron");
  var iron030 = action.input("Iron");
  var iron031 = action.input("Iron");
  var iron032 = action.input("Iron");
  var iron033 = action.input("Iron");
  var iron034 = action.input("Iron");
  var iron035 = action.input("Iron");
  var iron036 = action.input("Iron");
  var iron037 = action.input("Iron");
  var iron038 = action.input("Iron");
  var iron039 = action.input("Iron");
  var iron040 = action.input("Iron");
  var iron041 = action.input("Iron");
  var iron042 = action.input("Iron");
  var iron043 = action.input("Iron");
  var iron044 = action.input("Iron");
  var iron045 = action.input("Iron");
  var iron046 = action.input("Iron");
  var iron047 = action.input("Iron");
  var iron048 = action.input("Iron");
  var iron049 = action.input("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = action.random();
  ironbucket.update("key", key);
  var n = unsafe { ironbucket.n + 50 };
  action.st_sum(ironbucket.n, 50, n);
  ironbucket.update("n", n);
}

fn BucketOfIronTake1(action) {
  var iron000 = action.output("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = action.random();
  ironbucket.update("key", key);
  action.st_gt_eq(ironbucket.n, 1);
  var n = unsafe { ironbucket.n - 1 };
  action.st_sum(n, 1, ironbucket.n);
  ironbucket.update("n", n);
}

fn CraftStick(action) {
  var wood000 = action.input("Wood");
  var stick000 = action.output("Stick");
  var stick001 = action.output("Stick");
}

fn CraftWoodAxe(action) {
  var woodaxe = action.output("WoodAxe");
  woodaxe.set([["durability", 100]]);
  var wood000 = action.input("Wood");
  var stick000 = action.input("Stick");
  var stick001 = action.input("Stick");
}

fn CraftStoneAxe(action) {
  var woodaxe = action.input("WoodAxe");
  var stoneaxe = action.output("StoneAxe");
  stoneaxe.set([["durability", woodaxe.durability]]);
  var stone000 = action.input("Stone");
}

fn CraftIronAxe(action) {
  var stoneaxe = action.input("StoneAxe");
  var ironaxe = action.output("IronAxe");
  ironaxe.set([["durability", stoneaxe.durability]]);
  var iron000 = action.input("Iron");
}

fn UseWoodAxe(action) {
  var woodaxe = action.mutate("WoodAxe");
  action.st_gt(woodaxe.durability, 0);
  var durability = unsafe { woodaxe.durability - 1 };
  action.st_sum(durability, 1, woodaxe.durability);
  woodaxe.update("durability", durability);
  var wood000 = action.output("Wood");
  var wood001 = action.output("Wood");
  let target = action.top_limb_u256(8339694957348);
  var grind_key = action.pow_obj_grind(wood001, target);
  wood001.update("key", grind_key);
  action.intro_lt_eq_u256(wood001, target);
}

fn UseStoneAxe(action) {
  var stoneaxe = action.mutate("StoneAxe");
  action.st_gt(stoneaxe.durability, 0);
  var durability = unsafe { stoneaxe.durability - 1 };
  action.st_sum(durability, 1, stoneaxe.durability);
  stoneaxe.update("durability", durability);
  var wood000 = action.output("Wood");
  var wood001 = action.output("Wood");
  var wood002 = action.output("Wood");
  let target = action.top_limb_u256(8339694957348);
  var grind_key = action.pow_obj_grind(wood002, target);
  wood002.update("key", grind_key);
  action.intro_lt_eq_u256(wood002, target);
}

fn UseIronAxe(action) {
  var ironaxe = action.mutate("IronAxe");
  action.st_gt(ironaxe.durability, 0);
  var durability = unsafe { ironaxe.durability - 1 };
  action.st_sum(durability, 1, ironaxe.durability);
  ironaxe.update("durability", durability);
  var wood000 = action.output("Wood");
  var wood001 = action.output("Wood");
  var wood002 = action.output("Wood");
  var wood003 = action.output("Wood");
  let target = action.top_limb_u256(8339694957348);
  var grind_key = action.pow_obj_grind(wood003, target);
  wood003.update("key", grind_key);
  action.intro_lt_eq_u256(wood003, target);
}

fn UseQuarry1(action) {
  var quarry = action.mutate("Quarry");
  var key = action.random();
  quarry.update("key", key);
  action.st_equal(quarry.level, 1);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone001, target);
  stone001.update("key", grind_key);
  action.intro_lt_eq_u256(stone001, target);
}

fn UseMiningDrill1(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = action.random();
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 1);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var iron000 = action.output("Iron");
  var iron001 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron001, target);
  iron001.update("key", grind_key);
  action.intro_lt_eq_u256(iron001, target);
}

fn UseQuarry2(action) {
  var quarry = action.mutate("Quarry");
  var key = action.random();
  quarry.update("key", key);
  action.st_equal(quarry.level, 2);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone002, target);
  stone002.update("key", grind_key);
  action.intro_lt_eq_u256(stone002, target);
}

fn UseMiningDrill2(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = action.random();
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 2);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var iron000 = action.output("Iron");
  var iron001 = action.output("Iron");
  var iron002 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron002, target);
  iron002.update("key", grind_key);
  action.intro_lt_eq_u256(iron002, target);
}

fn UseQuarry3(action) {
  var quarry = action.mutate("Quarry");
  var key = action.random();
  quarry.update("key", key);
  action.st_equal(quarry.level, 3);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone003, target);
  stone003.update("key", grind_key);
  action.intro_lt_eq_u256(stone003, target);
}

fn UseMiningDrill3(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = action.random();
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 3);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var iron000 = action.output("Iron");
  var iron001 = action.output("Iron");
  var iron002 = action.output("Iron");
  var iron003 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron003, target);
  iron003.update("key", grind_key);
  action.intro_lt_eq_u256(iron003, target);
}

fn UseQuarry4(action) {
  var quarry = action.mutate("Quarry");
  var key = action.random();
  quarry.update("key", key);
  action.st_equal(quarry.level, 4);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone004, target);
  stone004.update("key", grind_key);
  action.intro_lt_eq_u256(stone004, target);
}

fn UseMiningDrill4(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = action.random();
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 4);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var iron000 = action.output("Iron");
  var iron001 = action.output("Iron");
  var iron002 = action.output("Iron");
  var iron003 = action.output("Iron");
  var iron004 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron004, target);
  iron004.update("key", grind_key);
  action.intro_lt_eq_u256(iron004, target);
}

fn UseQuarry5(action) {
  var quarry = action.mutate("Quarry");
  var key = action.random();
  quarry.update("key", key);
  action.st_equal(quarry.level, 5);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone005, target);
  stone005.update("key", grind_key);
  action.intro_lt_eq_u256(stone005, target);
}

fn UseMiningDrill5(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = action.random();
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 5);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var iron000 = action.output("Iron");
  var iron001 = action.output("Iron");
  var iron002 = action.output("Iron");
  var iron003 = action.output("Iron");
  var iron004 = action.output("Iron");
  var iron005 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron005, target);
  iron005.update("key", grind_key);
  action.intro_lt_eq_u256(iron005, target);
}

fn UseQuarry6(action) {
  var quarry = action.mutate("Quarry");
  var key = action.random();
  quarry.update("key", key);
  action.st_equal(quarry.level, 6);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  var stone006 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = action.pow_obj_grind(stone006, target);
  stone006.update("key", grind_key);
  action.intro_lt_eq_u256(stone006, target);
}

fn UseMiningDrill6(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = action.random();
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 6);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var iron000 = action.output("Iron");
  var iron001 = action.output("Iron");
  var iron002 = action.output("Iron");
  var iron003 = action.output("Iron");
  var iron004 = action.output("Iron");
  var iron005 = action.output("Iron");
  var iron006 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = action.pow_obj_grind(iron006, target);
  iron006.update("key", grind_key);
  action.intro_lt_eq_u256(iron006, target);
}

fn BuildQuarry(action) {
  var quarry = action.output("Quarry");
  quarry.set([["level", 1]]);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var wood005 = action.input("Wood");
  var wood006 = action.input("Wood");
  var wood007 = action.input("Wood");
  var wood008 = action.input("Wood");
  var wood009 = action.input("Wood");
  var wood010 = action.input("Wood");
  var wood011 = action.input("Wood");
  var wood012 = action.input("Wood");
  var wood013 = action.input("Wood");
  var wood014 = action.input("Wood");
  var wood015 = action.input("Wood");
  var wood016 = action.input("Wood");
  var wood017 = action.input("Wood");
  var wood018 = action.input("Wood");
  var wood019 = action.input("Wood");
  var stone000 = action.input("Stone");
  var stone001 = action.input("Stone");
  var stone002 = action.input("Stone");
  var stone003 = action.input("Stone");
  var stone004 = action.input("Stone");
  var stone005 = action.input("Stone");
  var stone006 = action.input("Stone");
  var stone007 = action.input("Stone");
  var stone008 = action.input("Stone");
  var stone009 = action.input("Stone");
}

fn BuildMiningDrill(action) {
  var miningdrill = action.output("MiningDrill");
  miningdrill.set([["level", 1]]);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var wood005 = action.input("Wood");
  var wood006 = action.input("Wood");
  var wood007 = action.input("Wood");
  var wood008 = action.input("Wood");
  var wood009 = action.input("Wood");
  var stone000 = action.input("Stone");
  var stone001 = action.input("Stone");
  var stone002 = action.input("Stone");
  var stone003 = action.input("Stone");
  var stone004 = action.input("Stone");
  var stone005 = action.input("Stone");
  var stone006 = action.input("Stone");
  var stone007 = action.input("Stone");
  var stone008 = action.input("Stone");
  var stone009 = action.input("Stone");
  var stone010 = action.input("Stone");
  var stone011 = action.input("Stone");
  var stone012 = action.input("Stone");
  var stone013 = action.input("Stone");
  var stone014 = action.input("Stone");
  var stone015 = action.input("Stone");
  var stone016 = action.input("Stone");
  var stone017 = action.input("Stone");
  var stone018 = action.input("Stone");
  var stone019 = action.input("Stone");
  var stone020 = action.input("Stone");
  var stone021 = action.input("Stone");
  var stone022 = action.input("Stone");
  var stone023 = action.input("Stone");
  var stone024 = action.input("Stone");
  var stone025 = action.input("Stone");
  var stone026 = action.input("Stone");
  var stone027 = action.input("Stone");
  var stone028 = action.input("Stone");
  var stone029 = action.input("Stone");
  var stone030 = action.input("Stone");
  var stone031 = action.input("Stone");
  var stone032 = action.input("Stone");
  var stone033 = action.input("Stone");
  var stone034 = action.input("Stone");
  var stone035 = action.input("Stone");
  var stone036 = action.input("Stone");
  var stone037 = action.input("Stone");
  var stone038 = action.input("Stone");
  var stone039 = action.input("Stone");
  var stone040 = action.input("Stone");
  var stone041 = action.input("Stone");
  var stone042 = action.input("Stone");
  var stone043 = action.input("Stone");
  var stone044 = action.input("Stone");
  var stone045 = action.input("Stone");
  var stone046 = action.input("Stone");
  var stone047 = action.input("Stone");
  var stone048 = action.input("Stone");
  var stone049 = action.input("Stone");
  var iron000 = action.input("Iron");
  var iron001 = action.input("Iron");
  var iron002 = action.input("Iron");
  var iron003 = action.input("Iron");
  var iron004 = action.input("Iron");
}

fn LevelUpQuarry1(action) {
  var quarry = action.mutate("Quarry");
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 40);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 20);
}

fn LevelUpMiningDrill1(action) {
  var miningdrill = action.mutate("MiningDrill");
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 20);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 100);
  var ironbucket = action.input("BucketOfIron");
  action.st_equal(ironbucket.n, 10);
}

fn LevelUpQuarry2(action) {
  var quarry = action.mutate("Quarry");
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 80);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 40);
}

fn LevelUpMiningDrill2(action) {
  var miningdrill = action.mutate("MiningDrill");
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 40);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 200);
  var ironbucket = action.input("BucketOfIron");
  action.st_equal(ironbucket.n, 20);
}

fn LevelUpQuarry3(action) {
  var quarry = action.mutate("Quarry");
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 160);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 80);
}

fn LevelUpMiningDrill3(action) {
  var miningdrill = action.mutate("MiningDrill");
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 80);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 400);
  var ironbucket = action.input("BucketOfIron");
  action.st_equal(ironbucket.n, 40);
}

fn LevelUpQuarry4(action) {
  var quarry = action.mutate("Quarry");
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 320);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 160);
}

fn LevelUpMiningDrill4(action) {
  var miningdrill = action.mutate("MiningDrill");
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 160);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 800);
  var ironbucket = action.input("BucketOfIron");
  action.st_equal(ironbucket.n, 80);
}

fn LevelUpQuarry5(action) {
  var quarry = action.mutate("Quarry");
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 640);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 320);
}

fn LevelUpMiningDrill5(action) {
  var miningdrill = action.mutate("MiningDrill");
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.input("BucketOfWood");
  action.st_equal(woodbucket.n, 320);
  var stonebucket = action.input("BucketOfStone");
  action.st_equal(stonebucket.n, 1600);
  var ironbucket = action.input("BucketOfIron");
  action.st_equal(ironbucket.n, 160);
}

fn BitsFromWood(action) {
  var wood000 = action.input("Wood");
  var bit000 = action.output("Bit");
  var work = action.intro_vdf(2, bit000);
  bit000.update("work", work);
}

fn BitsFromStone(action) {
  var stone000 = action.input("Stone");
  var bit000 = action.output("Bit");
  var bit001 = action.output("Bit");
  var work = action.intro_vdf(2, bit001);
  bit001.update("work", work);
}

fn BitsFromIron(action) {
  var iron000 = action.input("Iron");
  var bit000 = action.output("Bit");
  var bit001 = action.output("Bit");
  var bit002 = action.output("Bit");
  var bit003 = action.output("Bit");
  var bit004 = action.output("Bit");
  var work = action.intro_vdf(2, bit004);
  bit004.update("work", work);
}

