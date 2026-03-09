// pub mod examples;
pub mod predicates;
pub mod scenario;
pub mod sdk;
use std::sync::Arc;

use log::info;
use lt_eq_u256_pod::LtEqU256Pod;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder},
    lang::Module,
    middleware::{
        F, Key, MainPodProver, Params, Pod, RawValue, Statement, VDSet, Value,
        containers::Dictionary,
    },
};
use pod2utils::{macros::BuildContext, map, st_custom};
use txlib::{Object, StateRoot, Tx, TxBuilder, rekey};
use vdfpod::VdfPod;

fn prove(builder: MultiPodBuilder, prover: &dyn MainPodProver) -> MainPod {
    let solution = builder.solve().unwrap();
    log::debug!("solution needs {} pods", solution.solution().pod_count);
    solution.prove(prover).unwrap().pods.pop().unwrap()
}

#[derive(Debug)]
pub struct SpendableObject {
    pub pod: MainPod,
    pub obj: Dictionary,
    pub st_obj_idx: usize,
    pub st_tx_idx: usize,
    pub tx: Tx,
}

impl SpendableObject {
    pub fn tx_input(&self) -> (Dictionary, Tx) {
        (self.obj.clone(), self.tx.clone())
    }
}

pub struct SpendableObjects {
    pub pod: MainPod,
    pub objs: Vec<Dictionary>,
    pub tx: Tx,
}

impl SpendableObjects {
    pub fn obj(&self, index: usize) -> SpendableObject {
        SpendableObject {
            pod: self.pod.clone(),
            obj: self.objs[index].clone(),
            st_obj_idx: index,
            st_tx_idx: self.objs.len(),
            tx: self.tx.clone(),
        }
    }
    pub fn objs<const N: usize>(&self) -> [SpendableObject; N] {
        let objs: Vec<_> = (0..N).map(|i| self.obj(i)).collect();
        objs.try_into().unwrap()
    }
}

pub struct Helper {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    state_root: Arc<StateRoot>,
    prover: Box<dyn MainPodProver>,
    modules: Vec<Arc<Module>>, // [commit_mod, craft_mod]
}

impl Helper {
    const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

    pub fn new(mock: bool, state_root: Arc<StateRoot>) -> Self {
        let params = Params::default();
        let mock_prover = MockProver {};
        let real_prover = Prover {};
        let (vd_set, prover): (_, Box<dyn MainPodProver>) = if mock {
            (VDSet::new(&[]), Box::new(mock_prover))
        } else {
            let vd_set = &*DEFAULT_VD_SET;
            (vd_set.clone(), Box::new(real_prover))
        };

        let txlib_mod = Arc::new(txlib::predicates::module());
        let craftlib_mod = Arc::new(crate::predicates::module(txlib_mod.clone()));

        Self {
            mock,
            params,
            vd_set,
            state_root,
            prover,
            modules: vec![txlib_mod, craftlib_mod],
        }
    }

    fn new_builder(&self) -> MultiPodBuilder {
        MultiPodBuilder::new(&self.params, &self.vd_set)
    }
    fn new_tx_builder(&self, ctx: &mut BuildContext, inputs: &[(Dictionary, Tx)]) -> TxBuilder {
        TxBuilder::new(ctx, inputs, self.state_root.clone())
    }
    fn main_pod(&self, pod: Box<dyn Pod>) -> MainPod {
        let pub_statements = pod.pub_statements();
        MainPod {
            pod,
            public_statements: pub_statements,
            params: self.params.clone(),
        }
    }

    // Returns VdfPod, Vdf statement, work
    fn vdf(&self, n_iters: usize, input: RawValue) -> (MainPod, Statement, Value) {
        let vdf_pod = if self.mock {
            VdfPod::new_boxed_mock(&self.params, self.vd_set.clone(), n_iters, input)
        } else {
            VdfPod::new_boxed(&self.params, self.vd_set.clone(), n_iters, input)
        }
        .unwrap();
        let st_vdf = vdf_pod.pub_statements()[0].clone();
        let work = st_vdf.args()[2].literal().unwrap();
        (self.main_pod(vdf_pod), st_vdf, work)
    }

    // Returns LtEqU256Pod and LtEqU256 statement used to verify PoW.
    fn lt_eq_u256(&self, lhs: RawValue, rhs: RawValue) -> (MainPod, Statement) {
        let lt_eq_u256_pod = if self.mock {
            LtEqU256Pod::new_boxed_mock(&self.params, self.vd_set.clone(), lhs, rhs)
        } else {
            LtEqU256Pod::new_boxed(&self.params, self.vd_set.clone(), lhs, rhs)
        }
        .unwrap();
        let st_lt_eq_u256 = lt_eq_u256_pod.pub_statements()[0].clone();
        (self.main_pod(lt_eq_u256_pod), st_lt_eq_u256)
    }

    pub fn find_log(self) -> SpendableObjects {
        info!("finding log");
        let mut log = Object::new(map!({"blueprint" => "log"})).dict();
        let log0 = log.clone();
        let log0_raw = RawValue::from(log0.commitment());

        let (vdf_pod, st_vdf, work) = self.vdf(3, log0_raw);
        log.update(&Key::from("work"), &work).unwrap();

        let builder = self.new_builder();
        let mut ctx = BuildContext {
            builder,
            modules: self.modules.clone(),
        };
        let mut tx_builder = self.new_tx_builder(&mut ctx, &[]);
        ctx.builder.add_pod(vdf_pod).unwrap();
        let st_tx_insert_log = tx_builder.insert(&mut ctx, log.clone());

        let st_new_log = st_custom!(ctx,
            NewLog() = (
                DictContains(log0, "blueprint", "log"),
                st_vdf,
                DictUpdate(log, log0, "work", work),
                st_tx_insert_log
            ))
        .unwrap();
        let st = st_custom!(ctx,
            IsLog() = (
                st_new_log
            ))
        .unwrap();
        ctx.builder.reveal(&st).unwrap();
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_tx_finalize).unwrap();

        let pod = prove(ctx.builder, &*self.prover);
        pod.pod.verify().unwrap();
        SpendableObjects {
            objs: vec![log],
            pod,
            tx,
        }
    }

    pub fn craft_wood(self, log: SpendableObject) -> SpendableObjects {
        info!("crafting wood");
        let builder = self.new_builder();
        let mut ctx = BuildContext {
            builder,
            modules: self.modules.clone(),
        };
        let mut tx_builder = self.new_tx_builder(&mut ctx, &[log.tx_input()]);
        ctx.builder.add_pod(log.pod.clone()).unwrap();
        let log_pod_sts = log.pod.pod.pub_statements();
        let st_is_log = log_pod_sts[log.st_obj_idx].clone();
        let st_tx_delete_log = tx_builder.delete(&mut ctx, log.obj.clone());
        let mut wood = Object::new(map!({"blueprint" => "wood"})).dict();
        if !self.mock {
            while RawValue::from(wood.commitment()).0[3].0 > Self::WOOD_POW_DIFFICULTY {
                rekey(&mut wood);
            }
        }
        let wood_raw = RawValue::from(wood.commitment());
        let (lt_eq_u256_pod, st_lt_eq_u256) = self.lt_eq_u256(
            wood_raw,
            RawValue([F(0), F(0), F(0), F(Self::WOOD_POW_DIFFICULTY)]),
        );
        ctx.builder.add_pod(lt_eq_u256_pod).unwrap();
        let st_tx_insert_wood = tx_builder.insert(&mut ctx, wood.clone());

        let st_new_wood = st_custom!(ctx,
            NewWood() = (
                st_is_log,
                st_tx_delete_log,
                DictContains(wood, "blueprint", "wood"),
                st_lt_eq_u256,
                st_tx_insert_wood
            ))
        .unwrap();
        let st = st_custom!(ctx,
            IsWood() = (
                st_new_wood
            ))
        .unwrap();
        ctx.builder.reveal(&st).unwrap();
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_tx_finalize).unwrap();

        let pod = prove(ctx.builder, &*self.prover);
        pod.pod.verify().unwrap();
        SpendableObjects {
            objs: vec![wood],
            pod,
            tx,
        }
    }

    pub fn craft_sticks(self, wood: SpendableObject) -> SpendableObjects {
        info!("crafting sticks");
        let builder = self.new_builder();
        let mut ctx = BuildContext {
            builder,
            modules: self.modules.clone(),
        };
        let mut tx_builder = self.new_tx_builder(&mut ctx, &[wood.tx_input()]);
        ctx.builder.add_pod(wood.pod.clone()).unwrap();
        let wood_pod_sts = wood.pod.pod.pub_statements();
        let st_is_wood = wood_pod_sts[wood.st_obj_idx].clone();
        let st_tx_delete_wood = tx_builder.delete(&mut ctx, wood.obj.clone());

        let stick_a = Object::new(map!({"blueprint" => "stick"})).dict();
        let st_tx_insert_stick_a = tx_builder.insert(&mut ctx, stick_a.clone());
        let stick_b = Object::new(map!({"blueprint" => "stick"})).dict();
        let st_tx_insert_stick_b = tx_builder.insert(&mut ctx, stick_b.clone());

        let st_new_sticks = st_custom!(ctx,
            NewSticks() = (
                st_is_wood,
                st_tx_delete_wood,
                DictContains(stick_a, "blueprint", "stick"),
                st_tx_insert_stick_a,
                DictContains(stick_b, "blueprint", "stick"),
                st_tx_insert_stick_b
            ))
        .unwrap();
        let st_a = st_custom!(ctx,
            IsStick() = (
                st_new_sticks.clone(),
                Statement::None
            ))
        .unwrap();
        let st_b = st_custom!(ctx,
            IsStick() = (
                Statement::None,
                st_new_sticks
            ))
        .unwrap();
        ctx.builder.reveal(&st_a).unwrap();
        ctx.builder.reveal(&st_b).unwrap();
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_tx_finalize).unwrap();

        let pod = prove(ctx.builder, &*self.prover);
        pod.pod.verify().unwrap();
        SpendableObjects {
            objs: vec![stick_a, stick_b],
            pod,
            tx,
        }
    }

    pub fn craft_wood_pick(
        self,
        wood: SpendableObject,
        stick: SpendableObject,
    ) -> SpendableObjects {
        info!("crafting wood_pick");
        let builder = self.new_builder();
        let mut ctx = BuildContext {
            builder,
            modules: self.modules.clone(),
        };
        let mut tx_builder = self.new_tx_builder(&mut ctx, &[wood.tx_input(), stick.tx_input()]);
        ctx.builder.add_pod(wood.pod.clone()).unwrap();
        ctx.builder.add_pod(stick.pod.clone()).unwrap();
        let wood_pod_sts = wood.pod.pod.pub_statements();
        let st_is_wood = wood_pod_sts[wood.st_obj_idx].clone();
        let st_tx_delete_wood = tx_builder.delete(&mut ctx, wood.obj.clone());
        let stick_pod_sts = stick.pod.pod.pub_statements();
        let st_is_stick = stick_pod_sts[stick.st_obj_idx].clone();
        let st_tx_delete_stick = tx_builder.delete(&mut ctx, stick.obj.clone());

        let wood_pick = Object::new(map!({"blueprint" => "wood_pick", "durability" => 100})).dict();
        let st_tx_insert_wood_pick = tx_builder.insert(&mut ctx, wood_pick.clone());

        let st_new_wood_pick = st_custom!(ctx,
            NewWoodPick() = (
                st_is_wood,
                st_tx_delete_wood,
                st_is_stick,
                st_tx_delete_stick,
                DictContains(wood_pick, "blueprint", "wood_pick"),
                DictContains(wood_pick, "durability", 100),
                st_tx_insert_wood_pick
            ))
        .unwrap();
        let st = st_custom!(ctx,
            IsWoodPick() = (
                st_new_wood_pick,
                Statement::None
            ))
        .unwrap();
        ctx.builder.reveal(&st).unwrap();
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_tx_finalize).unwrap();

        let pod = prove(ctx.builder, &*self.prover);
        pod.pod.verify().unwrap();
        SpendableObjects {
            objs: vec![wood_pick],
            pod,
            tx,
        }
    }

    pub fn craft_stone_pick(
        self,
        stone: SpendableObject,
        stick: SpendableObject,
    ) -> SpendableObjects {
        info!("crafting stone_pick");
        let builder = self.new_builder();
        let mut ctx = BuildContext {
            builder,
            modules: self.modules.clone(),
        };
        let mut tx_builder = self.new_tx_builder(&mut ctx, &[stone.tx_input(), stick.tx_input()]);
        ctx.builder.add_pod(stone.pod.clone()).unwrap();
        ctx.builder.add_pod(stick.pod.clone()).unwrap();
        let stone_pod_sts = stone.pod.pod.pub_statements();
        let st_is_stone = stone_pod_sts[stone.st_obj_idx].clone();
        let st_tx_delete_stone = tx_builder.delete(&mut ctx, stone.obj.clone());
        let stick_pod_sts = stick.pod.pod.pub_statements();
        let st_is_stick = stick_pod_sts[stick.st_obj_idx].clone();
        let st_tx_delete_stick = tx_builder.delete(&mut ctx, stick.obj.clone());

        let stone_pick =
            Object::new(map!({"blueprint" => "stone_pick", "durability" => 200})).dict();
        let st_tx_insert_stone_pick = tx_builder.insert(&mut ctx, stone_pick.clone());

        let st_new_stone_pick = st_custom!(ctx,
            NewStonePick() = (
                st_is_stone,
                st_tx_delete_stone,
                st_is_stick,
                st_tx_delete_stick,
                DictContains(stone_pick, "blueprint", "stone_pick"),
                DictContains(stone_pick, "durability", 200),
                st_tx_insert_stone_pick
            ))
        .unwrap();
        let st = st_custom!(ctx,
            IsStonePick() = (
                st_new_stone_pick,
                Statement::None
            ))
        .unwrap();
        ctx.builder.reveal(&st).unwrap();
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_tx_finalize).unwrap();

        let pod = prove(ctx.builder, &*self.prover);
        pod.pod.verify().unwrap();
        SpendableObjects {
            objs: vec![stone_pick],
            pod,
            tx,
        }
    }

    pub fn mine_stone_with_wood_pick(self, wood_pick: SpendableObject) -> SpendableObjects {
        self.mine_stone_with_pick("wood_pick", wood_pick)
    }
    pub fn mine_stone_with_stone_pick(self, stone_pick: SpendableObject) -> SpendableObjects {
        self.mine_stone_with_pick("stone_pick", stone_pick)
    }

    fn mine_stone_with_pick(self, name: &str, pick: SpendableObject) -> SpendableObjects {
        info!("mining stone with {}", name);
        let pick0 = pick;
        let pick0_obj = pick0.obj.clone();
        let durability0 =
            i64::try_from(pick0_obj.get(&Key::from("durability")).unwrap().typed()).unwrap();
        let durability = durability0 - 1;
        let mut pick1_obj = pick0_obj.clone();
        pick1_obj
            .update(&Key::from("durability"), &Value::from(durability))
            .unwrap();
        let mut pick2_obj = pick1_obj.clone();
        rekey(&mut pick2_obj);
        let key = pick2_obj.get(&Key::from("key")).unwrap();
        let pick2_raw = RawValue::from(pick2_obj.commitment());
        let n_iters = match name {
            "wood_pick" => 10,
            "stone_pick" => 5,
            _ => unreachable!(),
        };
        let (vdf_pod, st_vdf, work) = self.vdf(n_iters, pick2_raw);
        let mut pick_obj = pick2_obj.clone();
        pick_obj.update(&Key::from("work"), &work).unwrap();

        let builder = self.new_builder();
        let mut ctx = BuildContext {
            builder,
            modules: self.modules.clone(),
        };
        let mut tx_builder = self.new_tx_builder(&mut ctx, &[pick0.tx_input()]);
        ctx.builder.add_pod(vdf_pod).unwrap();
        ctx.builder.add_pod(pick0.pod.clone()).unwrap();
        let pick_pod_sts = pick0.pod.pod.pub_statements();
        let st_is_pick = pick_pod_sts[pick0.st_obj_idx].clone();

        let st_tx_mutate_pick = tx_builder.mutate(&mut ctx, pick_obj.clone(), pick0.obj.clone());

        let st_used_pick = match name {
            "wood_pick" => {
                let st_used_pick_a = st_custom!(ctx,
                    UsedWoodPick_a() = (
                        SumOf((&pick0_obj, "durability"), durability, 1),
                        DictUpdate(pick1_obj, pick0_obj, "durability", durability),
                        DictUpdate(pick2_obj, pick1_obj, "key", key),
                        st_vdf,
                        DictUpdate(pick_obj, pick2_obj, "work", work)
                    ))
                .unwrap();
                st_custom!(ctx,
                    UsedWoodPick() = (
                        st_is_pick,
                        Gt((&pick0_obj, "durability"), 0),
                        st_used_pick_a,
                        st_tx_mutate_pick
                    ))
                .unwrap()
            }
            "stone_pick" => {
                let st_used_pick_a = st_custom!(ctx,
                    UsedStonePick_a() = (
                        SumOf((&pick0_obj, "durability"), durability, 1),
                        DictUpdate(pick1_obj, pick0_obj, "durability", durability),
                        DictUpdate(pick2_obj, pick1_obj, "key", key),
                        st_vdf,
                        DictUpdate(pick_obj, pick2_obj, "work", work)
                    ))
                .unwrap();
                st_custom!(ctx,
                    UsedStonePick() = (
                        st_is_pick,
                        Gt((&pick0_obj, "durability"), 0),
                        st_used_pick_a,
                        st_tx_mutate_pick
                    ))
                .unwrap()
            }
            _ => unreachable!(),
        };

        let st_is_pick = match name {
            "wood_pick" => st_custom!(ctx,
                    IsWoodPick() = (
                        Statement::None,
                        st_used_pick.clone()
                    ))
            .unwrap(),
            "stone_pick" => st_custom!(ctx,
                    IsStonePick() = (
                        Statement::None,
                        st_used_pick.clone()
                    ))
            .unwrap(),
            _ => unreachable!(),
        };

        let st_used_pick_for_stone = match name {
            "wood_pick" => st_custom!(ctx,
                    UsePickForStone() = (
                        st_used_pick,
                        Statement::None
                    ))
            .unwrap(),
            "stone_pick" => st_custom!(ctx,
                    UsePickForStone() = (
                        Statement::None,
                        st_used_pick
                    ))
            .unwrap(),
            _ => unreachable!(),
        };

        let stone = Object::new(map!({"blueprint" => "stone"})).dict();
        let st_tx_insert_stone = tx_builder.insert(&mut ctx, stone.clone());
        let st_new_stone = st_custom!(ctx,
            NewStone() = (
                st_used_pick_for_stone,
                DictContains(stone, "blueprint", "stone"),
                st_tx_insert_stone
            ))
        .unwrap();
        let st_is_stone = st_custom!(ctx,
            IsStone() = (
                st_new_stone.clone()
            ))
        .unwrap();

        ctx.builder.reveal(&st_is_pick).unwrap();
        ctx.builder.reveal(&st_is_stone).unwrap();
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx);
        ctx.builder.reveal(&st_tx_finalize).unwrap();

        let pod = prove(ctx.builder, &*self.prover);
        pod.pod.verify().unwrap();
        SpendableObjects {
            objs: vec![pick_obj, stone],
            pod,
            tx,
        }
    }
}

#[cfg(test)]
mod tests {

    use pod2utils::set;

    use super::*;

    fn update_state_root(state_root: &mut StateRoot, tx: &Tx) {
        state_root
            .transactions
            .insert(&Value::from(tx.dict()))
            .unwrap();
        for nullifier in tx.nullifiers.set() {
            state_root.transactions.insert(nullifier).unwrap();
        }
    }

    #[test]
    fn test_craft_helper() {
        let _ = env_logger::builder().try_init();
        let mock = true;

        let mut state_root = StateRoot {
            transactions: set!(),
            nullifiers: set!(),
        };
        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [log_a] = helper.find_log().objs();
        update_state_root(&mut state_root, &log_a.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [wood_a] = helper.craft_wood(log_a).objs();
        update_state_root(&mut state_root, &wood_a.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [stick_a, stick_b] = helper.craft_sticks(wood_a).objs();
        update_state_root(&mut state_root, &stick_a.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [log_b] = helper.find_log().objs();
        update_state_root(&mut state_root, &log_b.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [wood_b] = helper.craft_wood(log_b).objs();
        update_state_root(&mut state_root, &wood_b.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [wood_pick] = helper.craft_wood_pick(wood_b, stick_a).objs();
        update_state_root(&mut state_root, &wood_pick.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [wood_pick, stone_a] = helper.mine_stone_with_wood_pick(wood_pick).objs();
        update_state_root(&mut state_root, &wood_pick.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [stone_pick] = helper.craft_stone_pick(stone_a, stick_b).objs();
        update_state_root(&mut state_root, &stone_pick.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [stone_pick, _stone_b] = helper.mine_stone_with_stone_pick(stone_pick).objs();
        update_state_root(&mut state_root, &stone_pick.tx);

        let helper = Helper::new(mock, Arc::new(state_root.clone()));
        let [stone_pick, _stone_c] = helper.mine_stone_with_stone_pick(stone_pick).objs();
        update_state_root(&mut state_root, &stone_pick.tx);
    }
}
