fn CraftPocketknife(action) {
  var pocketknife = action.output("Pocketknife");
}

fn GatherWood(action) {
  var pocketknife = action.mutate("Pocketknife");
  var key = unsafe { action.random() };
  pocketknife.update("key", key);
  action.st_equal(1, 1);
  var wood000 = action.output("Wood");
  let target = action.top_limb_u256(8339694957348);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(wood000, target) };
  wood000.update("key", grind_key);
  action.intro_lt_eq_u256(wood000, target);
}

fn GatherStone(action) {
  var pocketknife = action.mutate("Pocketknife");
  var key = unsafe { action.random() };
  pocketknife.update("key", key);
  action.st_equal(1, 1);
  var stone000 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone000, target) };
  stone000.update("key", grind_key);
  action.intro_lt_eq_u256(stone000, target);
}

fn GatherIron(action) {
  var pocketknife = action.mutate("Pocketknife");
  var key = unsafe { action.random() };
  pocketknife.update("key", key);
  action.st_equal(1, 1);
  var iron000 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron000, target) };
  iron000.update("key", grind_key);
  action.intro_lt_eq_u256(iron000, target);
}

fn CraftBucketOfWood(action) {
  var iron000 = action.input("Iron");
  var woodbucket = action.output("BucketOfWood");
  woodbucket.set([["n", 0]]);
}

fn CraftBucketOfStone(action) {
  var iron000 = action.input("Iron");
  var stonebucket = action.output("BucketOfStone");
  stonebucket.set([["n", 0]]);
}

fn CraftBucketOfIron(action) {
  var iron000 = action.input("Iron");
  var ironbucket = action.output("BucketOfIron");
  ironbucket.set([["n", 0]]);
}

fn CraftBucketOfBits(action) {
  var iron000 = action.input("Iron");
  var bitbucket = action.output("BucketOfBits");
  bitbucket.set([["n", 0]]);
}

fn CraftBucketOfCoal(action) {
  var iron000 = action.input("Iron");
  var coalbucket = action.output("BucketOfCoal");
  coalbucket.set([["n", 0]]);
}

fn CraftBucketOfQuartz(action) {
  var iron000 = action.input("Iron");
  var quartzbucket = action.output("BucketOfQuartz");
  quartzbucket.set([["n", 0]]);
}

fn CraftBarrelOfOil(action) {
  var barrelofoil = action.output("BarrelOfOil");
  barrelofoil.set([["n", 0]]);
  var iron000 = action.input("Iron");
  var iron001 = action.input("Iron");
  var iron002 = action.input("Iron");
  var iron003 = action.input("Iron");
  var iron004 = action.input("Iron");
}

fn BucketOfWoodAdd1(action) {
  var wood000 = action.input("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
  woodbucket.update("key", key);
  var n = unsafe { woodbucket.n + 50 };
  action.st_sum(woodbucket.n, 50, n);
  woodbucket.update("n", n);
}

fn BucketOfWoodTake1(action) {
  var wood000 = action.output("Wood");
  var woodbucket = action.mutate("BucketOfWood");
  var key = unsafe { action.random() };
  woodbucket.update("key", key);
  action.st_gt_eq(woodbucket.n, 1);
  var n = unsafe { woodbucket.n - 1 };
  action.st_sum(n, 1, woodbucket.n);
  woodbucket.update("n", n);
}

fn BucketOfStoneAdd1(action) {
  var stone000 = action.input("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
  stonebucket.update("key", key);
  var n = unsafe { stonebucket.n + 50 };
  action.st_sum(stonebucket.n, 50, n);
  stonebucket.update("n", n);
}

fn BucketOfStoneTake1(action) {
  var stone000 = action.output("Stone");
  var stonebucket = action.mutate("BucketOfStone");
  var key = unsafe { action.random() };
  stonebucket.update("key", key);
  action.st_gt_eq(stonebucket.n, 1);
  var n = unsafe { stonebucket.n - 1 };
  action.st_sum(n, 1, stonebucket.n);
  stonebucket.update("n", n);
}

fn BucketOfIronAdd1(action) {
  var iron000 = action.input("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
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
  var key = unsafe { action.random() };
  ironbucket.update("key", key);
  var n = unsafe { ironbucket.n + 50 };
  action.st_sum(ironbucket.n, 50, n);
  ironbucket.update("n", n);
}

fn BucketOfIronTake1(action) {
  var iron000 = action.output("Iron");
  var ironbucket = action.mutate("BucketOfIron");
  var key = unsafe { action.random() };
  ironbucket.update("key", key);
  action.st_gt_eq(ironbucket.n, 1);
  var n = unsafe { ironbucket.n - 1 };
  action.st_sum(n, 1, ironbucket.n);
  ironbucket.update("n", n);
}

fn BucketOfBitsAdd1(action) {
  var bit000 = action.input("Bit");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 1 };
  action.st_sum(bitbucket.n, 1, n);
  bitbucket.update("n", n);
}

fn BucketOfBitsAdd5(action) {
  var bit000 = action.input("Bit");
  var bit001 = action.input("Bit");
  var bit002 = action.input("Bit");
  var bit003 = action.input("Bit");
  var bit004 = action.input("Bit");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 5 };
  action.st_sum(bitbucket.n, 5, n);
  bitbucket.update("n", n);
}

fn BucketOfBitsAdd10(action) {
  var bit000 = action.input("Bit");
  var bit001 = action.input("Bit");
  var bit002 = action.input("Bit");
  var bit003 = action.input("Bit");
  var bit004 = action.input("Bit");
  var bit005 = action.input("Bit");
  var bit006 = action.input("Bit");
  var bit007 = action.input("Bit");
  var bit008 = action.input("Bit");
  var bit009 = action.input("Bit");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 10 };
  action.st_sum(bitbucket.n, 10, n);
  bitbucket.update("n", n);
}

fn BucketOfBitsAdd50(action) {
  var bit000 = action.input("Bit");
  var bit001 = action.input("Bit");
  var bit002 = action.input("Bit");
  var bit003 = action.input("Bit");
  var bit004 = action.input("Bit");
  var bit005 = action.input("Bit");
  var bit006 = action.input("Bit");
  var bit007 = action.input("Bit");
  var bit008 = action.input("Bit");
  var bit009 = action.input("Bit");
  var bit010 = action.input("Bit");
  var bit011 = action.input("Bit");
  var bit012 = action.input("Bit");
  var bit013 = action.input("Bit");
  var bit014 = action.input("Bit");
  var bit015 = action.input("Bit");
  var bit016 = action.input("Bit");
  var bit017 = action.input("Bit");
  var bit018 = action.input("Bit");
  var bit019 = action.input("Bit");
  var bit020 = action.input("Bit");
  var bit021 = action.input("Bit");
  var bit022 = action.input("Bit");
  var bit023 = action.input("Bit");
  var bit024 = action.input("Bit");
  var bit025 = action.input("Bit");
  var bit026 = action.input("Bit");
  var bit027 = action.input("Bit");
  var bit028 = action.input("Bit");
  var bit029 = action.input("Bit");
  var bit030 = action.input("Bit");
  var bit031 = action.input("Bit");
  var bit032 = action.input("Bit");
  var bit033 = action.input("Bit");
  var bit034 = action.input("Bit");
  var bit035 = action.input("Bit");
  var bit036 = action.input("Bit");
  var bit037 = action.input("Bit");
  var bit038 = action.input("Bit");
  var bit039 = action.input("Bit");
  var bit040 = action.input("Bit");
  var bit041 = action.input("Bit");
  var bit042 = action.input("Bit");
  var bit043 = action.input("Bit");
  var bit044 = action.input("Bit");
  var bit045 = action.input("Bit");
  var bit046 = action.input("Bit");
  var bit047 = action.input("Bit");
  var bit048 = action.input("Bit");
  var bit049 = action.input("Bit");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 50 };
  action.st_sum(bitbucket.n, 50, n);
  bitbucket.update("n", n);
}

fn BucketOfBitsTake1(action) {
  var bit000 = action.output("Bit");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  action.st_gt_eq(bitbucket.n, 1);
  var n = unsafe { bitbucket.n - 1 };
  action.st_sum(n, 1, bitbucket.n);
  bitbucket.update("n", n);
}

fn BucketOfCoalAdd1(action) {
  var coal000 = action.input("Coal");
  var coalbucket = action.mutate("BucketOfCoal");
  var key = unsafe { action.random() };
  coalbucket.update("key", key);
  var n = unsafe { coalbucket.n + 1 };
  action.st_sum(coalbucket.n, 1, n);
  coalbucket.update("n", n);
}

fn BucketOfCoalAdd5(action) {
  var coal000 = action.input("Coal");
  var coal001 = action.input("Coal");
  var coal002 = action.input("Coal");
  var coal003 = action.input("Coal");
  var coal004 = action.input("Coal");
  var coalbucket = action.mutate("BucketOfCoal");
  var key = unsafe { action.random() };
  coalbucket.update("key", key);
  var n = unsafe { coalbucket.n + 5 };
  action.st_sum(coalbucket.n, 5, n);
  coalbucket.update("n", n);
}

fn BucketOfCoalAdd10(action) {
  var coal000 = action.input("Coal");
  var coal001 = action.input("Coal");
  var coal002 = action.input("Coal");
  var coal003 = action.input("Coal");
  var coal004 = action.input("Coal");
  var coal005 = action.input("Coal");
  var coal006 = action.input("Coal");
  var coal007 = action.input("Coal");
  var coal008 = action.input("Coal");
  var coal009 = action.input("Coal");
  var coalbucket = action.mutate("BucketOfCoal");
  var key = unsafe { action.random() };
  coalbucket.update("key", key);
  var n = unsafe { coalbucket.n + 10 };
  action.st_sum(coalbucket.n, 10, n);
  coalbucket.update("n", n);
}

fn BucketOfCoalAdd50(action) {
  var coal000 = action.input("Coal");
  var coal001 = action.input("Coal");
  var coal002 = action.input("Coal");
  var coal003 = action.input("Coal");
  var coal004 = action.input("Coal");
  var coal005 = action.input("Coal");
  var coal006 = action.input("Coal");
  var coal007 = action.input("Coal");
  var coal008 = action.input("Coal");
  var coal009 = action.input("Coal");
  var coal010 = action.input("Coal");
  var coal011 = action.input("Coal");
  var coal012 = action.input("Coal");
  var coal013 = action.input("Coal");
  var coal014 = action.input("Coal");
  var coal015 = action.input("Coal");
  var coal016 = action.input("Coal");
  var coal017 = action.input("Coal");
  var coal018 = action.input("Coal");
  var coal019 = action.input("Coal");
  var coal020 = action.input("Coal");
  var coal021 = action.input("Coal");
  var coal022 = action.input("Coal");
  var coal023 = action.input("Coal");
  var coal024 = action.input("Coal");
  var coal025 = action.input("Coal");
  var coal026 = action.input("Coal");
  var coal027 = action.input("Coal");
  var coal028 = action.input("Coal");
  var coal029 = action.input("Coal");
  var coal030 = action.input("Coal");
  var coal031 = action.input("Coal");
  var coal032 = action.input("Coal");
  var coal033 = action.input("Coal");
  var coal034 = action.input("Coal");
  var coal035 = action.input("Coal");
  var coal036 = action.input("Coal");
  var coal037 = action.input("Coal");
  var coal038 = action.input("Coal");
  var coal039 = action.input("Coal");
  var coal040 = action.input("Coal");
  var coal041 = action.input("Coal");
  var coal042 = action.input("Coal");
  var coal043 = action.input("Coal");
  var coal044 = action.input("Coal");
  var coal045 = action.input("Coal");
  var coal046 = action.input("Coal");
  var coal047 = action.input("Coal");
  var coal048 = action.input("Coal");
  var coal049 = action.input("Coal");
  var coalbucket = action.mutate("BucketOfCoal");
  var key = unsafe { action.random() };
  coalbucket.update("key", key);
  var n = unsafe { coalbucket.n + 50 };
  action.st_sum(coalbucket.n, 50, n);
  coalbucket.update("n", n);
}

fn BucketOfCoalTake1(action) {
  var coal000 = action.output("Coal");
  var coalbucket = action.mutate("BucketOfCoal");
  var key = unsafe { action.random() };
  coalbucket.update("key", key);
  action.st_gt_eq(coalbucket.n, 1);
  var n = unsafe { coalbucket.n - 1 };
  action.st_sum(n, 1, coalbucket.n);
  coalbucket.update("n", n);
}

fn BucketOfQuartzAdd1(action) {
  var quartz000 = action.input("Quartz");
  var quartzbucket = action.mutate("BucketOfQuartz");
  var key = unsafe { action.random() };
  quartzbucket.update("key", key);
  var n = unsafe { quartzbucket.n + 1 };
  action.st_sum(quartzbucket.n, 1, n);
  quartzbucket.update("n", n);
}

fn BucketOfQuartzAdd5(action) {
  var quartz000 = action.input("Quartz");
  var quartz001 = action.input("Quartz");
  var quartz002 = action.input("Quartz");
  var quartz003 = action.input("Quartz");
  var quartz004 = action.input("Quartz");
  var quartzbucket = action.mutate("BucketOfQuartz");
  var key = unsafe { action.random() };
  quartzbucket.update("key", key);
  var n = unsafe { quartzbucket.n + 5 };
  action.st_sum(quartzbucket.n, 5, n);
  quartzbucket.update("n", n);
}

fn BucketOfQuartzAdd10(action) {
  var quartz000 = action.input("Quartz");
  var quartz001 = action.input("Quartz");
  var quartz002 = action.input("Quartz");
  var quartz003 = action.input("Quartz");
  var quartz004 = action.input("Quartz");
  var quartz005 = action.input("Quartz");
  var quartz006 = action.input("Quartz");
  var quartz007 = action.input("Quartz");
  var quartz008 = action.input("Quartz");
  var quartz009 = action.input("Quartz");
  var quartzbucket = action.mutate("BucketOfQuartz");
  var key = unsafe { action.random() };
  quartzbucket.update("key", key);
  var n = unsafe { quartzbucket.n + 10 };
  action.st_sum(quartzbucket.n, 10, n);
  quartzbucket.update("n", n);
}

fn BucketOfQuartzAdd50(action) {
  var quartz000 = action.input("Quartz");
  var quartz001 = action.input("Quartz");
  var quartz002 = action.input("Quartz");
  var quartz003 = action.input("Quartz");
  var quartz004 = action.input("Quartz");
  var quartz005 = action.input("Quartz");
  var quartz006 = action.input("Quartz");
  var quartz007 = action.input("Quartz");
  var quartz008 = action.input("Quartz");
  var quartz009 = action.input("Quartz");
  var quartz010 = action.input("Quartz");
  var quartz011 = action.input("Quartz");
  var quartz012 = action.input("Quartz");
  var quartz013 = action.input("Quartz");
  var quartz014 = action.input("Quartz");
  var quartz015 = action.input("Quartz");
  var quartz016 = action.input("Quartz");
  var quartz017 = action.input("Quartz");
  var quartz018 = action.input("Quartz");
  var quartz019 = action.input("Quartz");
  var quartz020 = action.input("Quartz");
  var quartz021 = action.input("Quartz");
  var quartz022 = action.input("Quartz");
  var quartz023 = action.input("Quartz");
  var quartz024 = action.input("Quartz");
  var quartz025 = action.input("Quartz");
  var quartz026 = action.input("Quartz");
  var quartz027 = action.input("Quartz");
  var quartz028 = action.input("Quartz");
  var quartz029 = action.input("Quartz");
  var quartz030 = action.input("Quartz");
  var quartz031 = action.input("Quartz");
  var quartz032 = action.input("Quartz");
  var quartz033 = action.input("Quartz");
  var quartz034 = action.input("Quartz");
  var quartz035 = action.input("Quartz");
  var quartz036 = action.input("Quartz");
  var quartz037 = action.input("Quartz");
  var quartz038 = action.input("Quartz");
  var quartz039 = action.input("Quartz");
  var quartz040 = action.input("Quartz");
  var quartz041 = action.input("Quartz");
  var quartz042 = action.input("Quartz");
  var quartz043 = action.input("Quartz");
  var quartz044 = action.input("Quartz");
  var quartz045 = action.input("Quartz");
  var quartz046 = action.input("Quartz");
  var quartz047 = action.input("Quartz");
  var quartz048 = action.input("Quartz");
  var quartz049 = action.input("Quartz");
  var quartzbucket = action.mutate("BucketOfQuartz");
  var key = unsafe { action.random() };
  quartzbucket.update("key", key);
  var n = unsafe { quartzbucket.n + 50 };
  action.st_sum(quartzbucket.n, 50, n);
  quartzbucket.update("n", n);
}

fn BucketOfQuartzTake1(action) {
  var quartz000 = action.output("Quartz");
  var quartzbucket = action.mutate("BucketOfQuartz");
  var key = unsafe { action.random() };
  quartzbucket.update("key", key);
  action.st_gt_eq(quartzbucket.n, 1);
  var n = unsafe { quartzbucket.n - 1 };
  action.st_sum(n, 1, quartzbucket.n);
  quartzbucket.update("n", n);
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(wood001, target) };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(wood002, target) };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(wood003, target) };
  wood003.update("key", grind_key);
  action.intro_lt_eq_u256(wood003, target);
}

fn UseQuarry1(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 1);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone001, target) };
  stone001.update("key", grind_key);
  action.intro_lt_eq_u256(stone001, target);
}

fn UseMiningDrill1(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron001, target) };
  iron001.update("key", grind_key);
  action.intro_lt_eq_u256(iron001, target);
}

fn MineCoal1(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 1);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal000, target) };
  coal000.update("key", grind_key);
  action.intro_lt_eq_u256(coal000, target);
}

fn UseQuarry2(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 2);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone002, target) };
  stone002.update("key", grind_key);
  action.intro_lt_eq_u256(stone002, target);
}

fn UseMiningDrill2(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron002, target) };
  iron002.update("key", grind_key);
  action.intro_lt_eq_u256(iron002, target);
}

fn MineCoal2(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 2);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal001, target) };
  coal001.update("key", grind_key);
  action.intro_lt_eq_u256(coal001, target);
}

fn UseQuarry3(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 3);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone003, target) };
  stone003.update("key", grind_key);
  action.intro_lt_eq_u256(stone003, target);
}

fn UseMiningDrill3(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron003, target) };
  iron003.update("key", grind_key);
  action.intro_lt_eq_u256(iron003, target);
}

fn MineCoal3(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 3);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal002, target) };
  coal002.update("key", grind_key);
  action.intro_lt_eq_u256(coal002, target);
}

fn UseQuarry4(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 4);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone004, target) };
  stone004.update("key", grind_key);
  action.intro_lt_eq_u256(stone004, target);
}

fn UseMiningDrill4(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron004, target) };
  iron004.update("key", grind_key);
  action.intro_lt_eq_u256(iron004, target);
}

fn MineCoal4(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 4);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal003, target) };
  coal003.update("key", grind_key);
  action.intro_lt_eq_u256(coal003, target);
}

fn UseQuarry5(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 5);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone005, target) };
  stone005.update("key", grind_key);
  action.intro_lt_eq_u256(stone005, target);
}

fn UseMiningDrill5(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron005, target) };
  iron005.update("key", grind_key);
  action.intro_lt_eq_u256(iron005, target);
}

fn MineCoal5(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 5);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  var coal004 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal004, target) };
  coal004.update("key", grind_key);
  action.intro_lt_eq_u256(coal004, target);
}

fn UseQuarry6(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone006, target) };
  stone006.update("key", grind_key);
  action.intro_lt_eq_u256(stone006, target);
}

fn UseMiningDrill6(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
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
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron006, target) };
  iron006.update("key", grind_key);
  action.intro_lt_eq_u256(iron006, target);
}

fn MineCoal6(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 6);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  var coal004 = action.output("Coal");
  var coal005 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal005, target) };
  coal005.update("key", grind_key);
  action.intro_lt_eq_u256(coal005, target);
}

fn UseQuarry7(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 7);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  var stone006 = action.output("Stone");
  var stone007 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone007, target) };
  stone007.update("key", grind_key);
  action.intro_lt_eq_u256(stone007, target);
}

fn UseMiningDrill7(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 7);
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
  var iron007 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron007, target) };
  iron007.update("key", grind_key);
  action.intro_lt_eq_u256(iron007, target);
}

fn MineCoal7(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 7);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  var coal004 = action.output("Coal");
  var coal005 = action.output("Coal");
  var coal006 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal006, target) };
  coal006.update("key", grind_key);
  action.intro_lt_eq_u256(coal006, target);
}

fn UseQuarry8(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 8);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  var stone006 = action.output("Stone");
  var stone007 = action.output("Stone");
  var stone008 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone008, target) };
  stone008.update("key", grind_key);
  action.intro_lt_eq_u256(stone008, target);
}

fn UseMiningDrill8(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 8);
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
  var iron007 = action.output("Iron");
  var iron008 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron008, target) };
  iron008.update("key", grind_key);
  action.intro_lt_eq_u256(iron008, target);
}

fn MineCoal8(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 8);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  var coal004 = action.output("Coal");
  var coal005 = action.output("Coal");
  var coal006 = action.output("Coal");
  var coal007 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal007, target) };
  coal007.update("key", grind_key);
  action.intro_lt_eq_u256(coal007, target);
}

fn UseQuarry9(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 9);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  var stone006 = action.output("Stone");
  var stone007 = action.output("Stone");
  var stone008 = action.output("Stone");
  var stone009 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone009, target) };
  stone009.update("key", grind_key);
  action.intro_lt_eq_u256(stone009, target);
}

fn UseMiningDrill9(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 9);
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
  var iron007 = action.output("Iron");
  var iron008 = action.output("Iron");
  var iron009 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron009, target) };
  iron009.update("key", grind_key);
  action.intro_lt_eq_u256(iron009, target);
}

fn MineCoal9(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 9);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  var coal004 = action.output("Coal");
  var coal005 = action.output("Coal");
  var coal006 = action.output("Coal");
  var coal007 = action.output("Coal");
  var coal008 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal008, target) };
  coal008.update("key", grind_key);
  action.intro_lt_eq_u256(coal008, target);
}

fn UseQuarry10(action) {
  var quarry = action.mutate("Quarry");
  var key = unsafe { action.random() };
  quarry.update("key", key);
  action.st_equal(quarry.level, 10);
  var stone000 = action.output("Stone");
  var stone001 = action.output("Stone");
  var stone002 = action.output("Stone");
  var stone003 = action.output("Stone");
  var stone004 = action.output("Stone");
  var stone005 = action.output("Stone");
  var stone006 = action.output("Stone");
  var stone007 = action.output("Stone");
  var stone008 = action.output("Stone");
  var stone009 = action.output("Stone");
  var stone010 = action.output("Stone");
  let target = action.top_limb_u256(5040314377883);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(stone010, target) };
  stone010.update("key", grind_key);
  action.intro_lt_eq_u256(stone010, target);
}

fn UseMiningDrill10(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 10);
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
  var iron007 = action.output("Iron");
  var iron008 = action.output("Iron");
  var iron009 = action.output("Iron");
  var iron010 = action.output("Iron");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(iron010, target) };
  iron010.update("key", grind_key);
  action.intro_lt_eq_u256(iron010, target);
}

fn MineCoal10(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 10);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var coal000 = action.output("Coal");
  var coal001 = action.output("Coal");
  var coal002 = action.output("Coal");
  var coal003 = action.output("Coal");
  var coal004 = action.output("Coal");
  var coal005 = action.output("Coal");
  var coal006 = action.output("Coal");
  var coal007 = action.output("Coal");
  var coal008 = action.output("Coal");
  var coal009 = action.output("Coal");
  let target = action.top_limb_u256(1740933798417);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(coal009, target) };
  coal009.update("key", grind_key);
  action.intro_lt_eq_u256(coal009, target);
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

fn BuildPumpjack(action) {
  var pumpjack = action.output("Pumpjack");
  pumpjack.set([["level", 1]]);
  var quartz000 = action.input("Quartz");
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 50);
  var stonebucket_n = unsafe { stonebucket.n - 50 };
  action.st_sum(stonebucket_n, 50, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 100);
  var ironbucket_n = unsafe { ironbucket.n - 100 };
  action.st_sum(ironbucket_n, 100, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpQuarry1(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 1);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 40);
  var woodbucket_n = unsafe { woodbucket.n - 40 };
  action.st_sum(woodbucket_n, 40, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 20);
  var stonebucket_n = unsafe { stonebucket.n - 20 };
  action.st_sum(stonebucket_n, 20, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill1(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 1);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 20);
  var woodbucket_n = unsafe { woodbucket.n - 20 };
  action.st_sum(woodbucket_n, 20, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 100);
  var stonebucket_n = unsafe { stonebucket.n - 100 };
  action.st_sum(stonebucket_n, 100, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 10);
  var ironbucket_n = unsafe { ironbucket.n - 10 };
  action.st_sum(ironbucket_n, 10, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack1(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 1);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 100);
  var stonebucket_n = unsafe { stonebucket.n - 100 };
  action.st_sum(stonebucket_n, 100, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 200);
  var ironbucket_n = unsafe { ironbucket.n - 200 };
  action.st_sum(ironbucket_n, 200, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 2);
  var quartzbucket_n = unsafe { quartzbucket.n - 2 };
  action.st_sum(quartzbucket_n, 2, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry2(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 2);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 80);
  var woodbucket_n = unsafe { woodbucket.n - 80 };
  action.st_sum(woodbucket_n, 80, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 40);
  var stonebucket_n = unsafe { stonebucket.n - 40 };
  action.st_sum(stonebucket_n, 40, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill2(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 2);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 40);
  var woodbucket_n = unsafe { woodbucket.n - 40 };
  action.st_sum(woodbucket_n, 40, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 200);
  var stonebucket_n = unsafe { stonebucket.n - 200 };
  action.st_sum(stonebucket_n, 200, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 20);
  var ironbucket_n = unsafe { ironbucket.n - 20 };
  action.st_sum(ironbucket_n, 20, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack2(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 2);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 200);
  var stonebucket_n = unsafe { stonebucket.n - 200 };
  action.st_sum(stonebucket_n, 200, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 400);
  var ironbucket_n = unsafe { ironbucket.n - 400 };
  action.st_sum(ironbucket_n, 400, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 4);
  var quartzbucket_n = unsafe { quartzbucket.n - 4 };
  action.st_sum(quartzbucket_n, 4, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry3(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 3);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 160);
  var woodbucket_n = unsafe { woodbucket.n - 160 };
  action.st_sum(woodbucket_n, 160, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 80);
  var stonebucket_n = unsafe { stonebucket.n - 80 };
  action.st_sum(stonebucket_n, 80, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill3(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 3);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 80);
  var woodbucket_n = unsafe { woodbucket.n - 80 };
  action.st_sum(woodbucket_n, 80, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 400);
  var stonebucket_n = unsafe { stonebucket.n - 400 };
  action.st_sum(stonebucket_n, 400, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 40);
  var ironbucket_n = unsafe { ironbucket.n - 40 };
  action.st_sum(ironbucket_n, 40, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack3(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 3);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 400);
  var stonebucket_n = unsafe { stonebucket.n - 400 };
  action.st_sum(stonebucket_n, 400, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 800);
  var ironbucket_n = unsafe { ironbucket.n - 800 };
  action.st_sum(ironbucket_n, 800, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 8);
  var quartzbucket_n = unsafe { quartzbucket.n - 8 };
  action.st_sum(quartzbucket_n, 8, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry4(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 4);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 320);
  var woodbucket_n = unsafe { woodbucket.n - 320 };
  action.st_sum(woodbucket_n, 320, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 160);
  var stonebucket_n = unsafe { stonebucket.n - 160 };
  action.st_sum(stonebucket_n, 160, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill4(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 4);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 160);
  var woodbucket_n = unsafe { woodbucket.n - 160 };
  action.st_sum(woodbucket_n, 160, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 800);
  var stonebucket_n = unsafe { stonebucket.n - 800 };
  action.st_sum(stonebucket_n, 800, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 80);
  var ironbucket_n = unsafe { ironbucket.n - 80 };
  action.st_sum(ironbucket_n, 80, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack4(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 4);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 800);
  var stonebucket_n = unsafe { stonebucket.n - 800 };
  action.st_sum(stonebucket_n, 800, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 1600);
  var ironbucket_n = unsafe { ironbucket.n - 1600 };
  action.st_sum(ironbucket_n, 1600, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 16);
  var quartzbucket_n = unsafe { quartzbucket.n - 16 };
  action.st_sum(quartzbucket_n, 16, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry5(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 5);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 640);
  var woodbucket_n = unsafe { woodbucket.n - 640 };
  action.st_sum(woodbucket_n, 640, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 320);
  var stonebucket_n = unsafe { stonebucket.n - 320 };
  action.st_sum(stonebucket_n, 320, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill5(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 5);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 320);
  var woodbucket_n = unsafe { woodbucket.n - 320 };
  action.st_sum(woodbucket_n, 320, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 1600);
  var stonebucket_n = unsafe { stonebucket.n - 1600 };
  action.st_sum(stonebucket_n, 1600, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 160);
  var ironbucket_n = unsafe { ironbucket.n - 160 };
  action.st_sum(ironbucket_n, 160, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack5(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 5);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 1600);
  var stonebucket_n = unsafe { stonebucket.n - 1600 };
  action.st_sum(stonebucket_n, 1600, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 3200);
  var ironbucket_n = unsafe { ironbucket.n - 3200 };
  action.st_sum(ironbucket_n, 3200, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 32);
  var quartzbucket_n = unsafe { quartzbucket.n - 32 };
  action.st_sum(quartzbucket_n, 32, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry6(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 6);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 1280);
  var woodbucket_n = unsafe { woodbucket.n - 1280 };
  action.st_sum(woodbucket_n, 1280, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 640);
  var stonebucket_n = unsafe { stonebucket.n - 640 };
  action.st_sum(stonebucket_n, 640, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill6(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 6);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 640);
  var woodbucket_n = unsafe { woodbucket.n - 640 };
  action.st_sum(woodbucket_n, 640, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 3200);
  var stonebucket_n = unsafe { stonebucket.n - 3200 };
  action.st_sum(stonebucket_n, 3200, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 320);
  var ironbucket_n = unsafe { ironbucket.n - 320 };
  action.st_sum(ironbucket_n, 320, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack6(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 6);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 3200);
  var stonebucket_n = unsafe { stonebucket.n - 3200 };
  action.st_sum(stonebucket_n, 3200, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 6400);
  var ironbucket_n = unsafe { ironbucket.n - 6400 };
  action.st_sum(ironbucket_n, 6400, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 64);
  var quartzbucket_n = unsafe { quartzbucket.n - 64 };
  action.st_sum(quartzbucket_n, 64, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry7(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 7);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 2560);
  var woodbucket_n = unsafe { woodbucket.n - 2560 };
  action.st_sum(woodbucket_n, 2560, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 1280);
  var stonebucket_n = unsafe { stonebucket.n - 1280 };
  action.st_sum(stonebucket_n, 1280, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill7(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 7);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 1280);
  var woodbucket_n = unsafe { woodbucket.n - 1280 };
  action.st_sum(woodbucket_n, 1280, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 6400);
  var stonebucket_n = unsafe { stonebucket.n - 6400 };
  action.st_sum(stonebucket_n, 6400, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 640);
  var ironbucket_n = unsafe { ironbucket.n - 640 };
  action.st_sum(ironbucket_n, 640, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack7(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 7);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 6400);
  var stonebucket_n = unsafe { stonebucket.n - 6400 };
  action.st_sum(stonebucket_n, 6400, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 12800);
  var ironbucket_n = unsafe { ironbucket.n - 12800 };
  action.st_sum(ironbucket_n, 12800, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 128);
  var quartzbucket_n = unsafe { quartzbucket.n - 128 };
  action.st_sum(quartzbucket_n, 128, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry8(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 8);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 5120);
  var woodbucket_n = unsafe { woodbucket.n - 5120 };
  action.st_sum(woodbucket_n, 5120, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 2560);
  var stonebucket_n = unsafe { stonebucket.n - 2560 };
  action.st_sum(stonebucket_n, 2560, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill8(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 8);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 2560);
  var woodbucket_n = unsafe { woodbucket.n - 2560 };
  action.st_sum(woodbucket_n, 2560, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 12800);
  var stonebucket_n = unsafe { stonebucket.n - 12800 };
  action.st_sum(stonebucket_n, 12800, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 1280);
  var ironbucket_n = unsafe { ironbucket.n - 1280 };
  action.st_sum(ironbucket_n, 1280, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack8(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 8);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 12800);
  var stonebucket_n = unsafe { stonebucket.n - 12800 };
  action.st_sum(stonebucket_n, 12800, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 25600);
  var ironbucket_n = unsafe { ironbucket.n - 25600 };
  action.st_sum(ironbucket_n, 25600, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 256);
  var quartzbucket_n = unsafe { quartzbucket.n - 256 };
  action.st_sum(quartzbucket_n, 256, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn LevelUpQuarry9(action) {
  var quarry = action.mutate("Quarry");
  action.st_equal(quarry.level, 9);
  var level = unsafe { quarry.level + 1 };
  action.st_sum(1, quarry.level, level);
  quarry.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 10240);
  var woodbucket_n = unsafe { woodbucket.n - 10240 };
  action.st_sum(woodbucket_n, 10240, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 5120);
  var stonebucket_n = unsafe { stonebucket.n - 5120 };
  action.st_sum(stonebucket_n, 5120, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
}

fn LevelUpMiningDrill9(action) {
  var miningdrill = action.mutate("MiningDrill");
  action.st_equal(miningdrill.level, 9);
  var level = unsafe { miningdrill.level + 1 };
  action.st_sum(1, miningdrill.level, level);
  miningdrill.update("level", level);
  var woodbucket = action.mutate("BucketOfWood");
  action.st_gt_eq(woodbucket.n, 5120);
  var woodbucket_n = unsafe { woodbucket.n - 5120 };
  action.st_sum(woodbucket_n, 5120, woodbucket.n);
  woodbucket.update("n", woodbucket_n);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 25600);
  var stonebucket_n = unsafe { stonebucket.n - 25600 };
  action.st_sum(stonebucket_n, 25600, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 2560);
  var ironbucket_n = unsafe { ironbucket.n - 2560 };
  action.st_sum(ironbucket_n, 2560, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
}

fn LevelUpPumpjack9(action) {
  var pumpjack = action.mutate("Pumpjack");
  action.st_equal(pumpjack.level, 9);
  var level = unsafe { pumpjack.level + 1 };
  action.st_sum(1, pumpjack.level, level);
  pumpjack.update("level", level);
  var stonebucket = action.mutate("BucketOfStone");
  action.st_gt_eq(stonebucket.n, 25600);
  var stonebucket_n = unsafe { stonebucket.n - 25600 };
  action.st_sum(stonebucket_n, 25600, stonebucket.n);
  stonebucket.update("n", stonebucket_n);
  var ironbucket = action.mutate("BucketOfIron");
  action.st_gt_eq(ironbucket.n, 51200);
  var ironbucket_n = unsafe { ironbucket.n - 51200 };
  action.st_sum(ironbucket_n, 51200, ironbucket.n);
  ironbucket.update("n", ironbucket_n);
  var quartzbucket = action.mutate("BucketOfQuartz");
  action.st_gt_eq(quartzbucket.n, 512);
  var quartzbucket_n = unsafe { quartzbucket.n - 512 };
  action.st_sum(quartzbucket_n, 512, quartzbucket.n);
  quartzbucket.update("n", quartzbucket_n);
}

fn PumpOil(action) {
  var pumpjack = action.mutate("Pumpjack");
  var key = unsafe { action.random() };
  pumpjack.update("key", key);
  var coal000 = action.input("Coal");
  var coal001 = action.input("Coal");
  var coal002 = action.input("Coal");
  var coal003 = action.input("Coal");
  var coal004 = action.input("Coal");
  var barrelofoil = action.mutate("BarrelOfOil");
  var n_steps = unsafe { arg("n_steps") ?? 1 };
  action.st_gt_eq(n_steps, 1);
  var pumped = unsafe { n_steps * pumpjack.level };
  action.st_product(n_steps, pumpjack.level, pumped);
  var n = unsafe { barrelofoil.n + pumped };
  action.st_sum(barrelofoil.n, pumped, n);
  barrelofoil.update("n", n);
  var vdf_steps = unsafe { n_steps * 50 };
  action.st_product(n_steps, 50, vdf_steps);
  var work = action.intro_vdf(vdf_steps, barrelofoil);
}

fn MineGem1(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 1);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(509014584)], ["sapphire", action.top_limb_u256(31813411)], ["emerald", action.top_limb_u256(1988338)], ["ruby", action.top_limb_u256(124271)], ["diamond", action.top_limb_u256(7766)]]);
  let target = action.top_limb_u256(8144233356);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem2(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 2);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(1018029169)], ["sapphire", action.top_limb_u256(63626823)], ["emerald", action.top_limb_u256(3976676)], ["ruby", action.top_limb_u256(248542)], ["diamond", action.top_limb_u256(15533)]]);
  let target = action.top_limb_u256(16288466713);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem3(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 3);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(1527043754)], ["sapphire", action.top_limb_u256(95440234)], ["emerald", action.top_limb_u256(5965014)], ["ruby", action.top_limb_u256(372813)], ["diamond", action.top_limb_u256(23300)]]);
  let target = action.top_limb_u256(24432700070);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem4(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 4);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(2036058339)], ["sapphire", action.top_limb_u256(127253646)], ["emerald", action.top_limb_u256(7953352)], ["ruby", action.top_limb_u256(497084)], ["diamond", action.top_limb_u256(31067)]]);
  let target = action.top_limb_u256(32576933427);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem5(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 5);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(2545072923)], ["sapphire", action.top_limb_u256(159067057)], ["emerald", action.top_limb_u256(9941691)], ["ruby", action.top_limb_u256(621355)], ["diamond", action.top_limb_u256(38834)]]);
  let target = action.top_limb_u256(40721166783);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem6(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 6);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(3054087508)], ["sapphire", action.top_limb_u256(190880469)], ["emerald", action.top_limb_u256(11930029)], ["ruby", action.top_limb_u256(745626)], ["diamond", action.top_limb_u256(46601)]]);
  let target = action.top_limb_u256(48865400140);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem7(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 7);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(3563102093)], ["sapphire", action.top_limb_u256(222693880)], ["emerald", action.top_limb_u256(13918367)], ["ruby", action.top_limb_u256(869897)], ["diamond", action.top_limb_u256(54368)]]);
  let target = action.top_limb_u256(57009633497);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem8(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 8);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(4072116678)], ["sapphire", action.top_limb_u256(254507292)], ["emerald", action.top_limb_u256(15906705)], ["ruby", action.top_limb_u256(994169)], ["diamond", action.top_limb_u256(62135)]]);
  let target = action.top_limb_u256(65153866854);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem9(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 9);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(4581131263)], ["sapphire", action.top_limb_u256(286320703)], ["emerald", action.top_limb_u256(17895043)], ["ruby", action.top_limb_u256(1118440)], ["diamond", action.top_limb_u256(69902)]]);
  let target = action.top_limb_u256(73298100211);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn MineGem10(action) {
  var miningdrill = action.mutate("MiningDrill");
  var key = unsafe { action.random() };
  miningdrill.update("key", key);
  action.st_equal(miningdrill.level, 10);
  var wood000 = action.input("Wood");
  var wood001 = action.input("Wood");
  var wood002 = action.input("Wood");
  var wood003 = action.input("Wood");
  var wood004 = action.input("Wood");
  var geode000 = action.output("Geode");
  geode000.set([["amethyst", action.top_limb_u256(5090145847)], ["sapphire", action.top_limb_u256(318134115)], ["emerald", action.top_limb_u256(19883382)], ["ruby", action.top_limb_u256(1242711)], ["diamond", action.top_limb_u256(77669)]]);
  let target = action.top_limb_u256(81442333567);
  var grind_key = unsafe { arg("grind_key") ?? action.pow_obj_grind(geode000, target) };
  geode000.update("key", grind_key);
  action.intro_lt_eq_u256(geode000, target);
}

fn CrackGeodeQuartz(action) {
  var geode000 = action.input("Geode");
  var quartz000 = action.output("Quartz");
  action.intro_lt_eq_u256(geode000.amethyst, geode000.stable_identifier);
}

fn CrackGeodeAmethyst(action) {
  var geode000 = action.input("Geode");
  var amethyst000 = action.output("Amethyst");
  action.intro_lt_eq_u256(geode000.stable_identifier, geode000.amethyst);
  action.intro_lt_eq_u256(geode000.sapphire, geode000.stable_identifier);
}

fn CrackGeodeSapphire(action) {
  var geode000 = action.input("Geode");
  var sapphire000 = action.output("Sapphire");
  action.intro_lt_eq_u256(geode000.stable_identifier, geode000.sapphire);
  action.intro_lt_eq_u256(geode000.emerald, geode000.stable_identifier);
}

fn CrackGeodeEmerald(action) {
  var geode000 = action.input("Geode");
  var emerald000 = action.output("Emerald");
  action.intro_lt_eq_u256(geode000.stable_identifier, geode000.emerald);
  action.intro_lt_eq_u256(geode000.ruby, geode000.stable_identifier);
}

fn CrackGeodeRuby(action) {
  var geode000 = action.input("Geode");
  var ruby000 = action.output("Ruby");
  action.intro_lt_eq_u256(geode000.stable_identifier, geode000.ruby);
  action.intro_lt_eq_u256(geode000.diamond, geode000.stable_identifier);
}

fn CrackGeodeDiamond(action) {
  var geode000 = action.input("Geode");
  var diamond000 = action.output("Diamond");
  action.intro_lt_eq_u256(geode000.stable_identifier, geode000.diamond);
}

fn BitsFromWood(action) {
  var wood000 = action.input("Wood");
  var bit000 = action.output("Bit");
  var work = action.intro_vdf(2, bit000);
}

fn BitsFromStone(action) {
  var stone000 = action.input("Stone");
  var bit000 = action.output("Bit");
  var bit001 = action.output("Bit");
  var work = action.intro_vdf(2, bit001);
}

fn BitsFromIron(action) {
  var iron000 = action.input("Iron");
  var bit000 = action.output("Bit");
  var bit001 = action.output("Bit");
  var bit002 = action.output("Bit");
  var bit003 = action.output("Bit");
  var bit004 = action.output("Bit");
  var work = action.intro_vdf(2, bit004);
}

fn BitsFromQuartz(action) {
  var quartz000 = action.input("Quartz");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 100 };
  action.st_sum(bitbucket.n, 100, n);
  bitbucket.update("n", n);
}

fn BitsFromAmethyst(action) {
  var amethyst000 = action.input("Amethyst");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 2000 };
  action.st_sum(bitbucket.n, 2000, n);
  bitbucket.update("n", n);
}

fn BitsFromSapphire(action) {
  var sapphire000 = action.input("Sapphire");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 40000 };
  action.st_sum(bitbucket.n, 40000, n);
  bitbucket.update("n", n);
}

fn BitsFromEmerald(action) {
  var emerald000 = action.input("Emerald");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 800000 };
  action.st_sum(bitbucket.n, 800000, n);
  bitbucket.update("n", n);
}

fn BitsFromRuby(action) {
  var ruby000 = action.input("Ruby");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 16000000 };
  action.st_sum(bitbucket.n, 16000000, n);
  bitbucket.update("n", n);
}

fn BitsFromDiamond(action) {
  var diamond000 = action.input("Diamond");
  var bitbucket = action.mutate("BucketOfBits");
  var key = unsafe { action.random() };
  bitbucket.update("key", key);
  var n = unsafe { bitbucket.n + 320000000 };
  action.st_sum(bitbucket.n, 320000000, n);
  bitbucket.update("n", n);
}

