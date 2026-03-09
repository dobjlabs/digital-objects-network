use std::sync::Arc;

use pod2::lang::{self, Module, load_module};

pub fn module(txlib_mod: Arc<Module>) -> lang::Module {
    let params = pod2::middleware::Params::default();
    load_module(
        include_str!("craftlib.podlang"),
        "craft",
        &params,
        &[txlib_mod],
    )
    .expect("compiles")
}

#[cfg(test)]
mod tests {
    use pod2::lang::PrettyPrint;

    use super::*;

    #[test]
    fn test_predicates_exist() {
        let txlib_mod = Arc::new(txlib::predicates::module());
        let module = module(txlib_mod);
        println!("craftlib id: {:#}", module.batch.id());
        module.predicate_ref_by_name("IsLog").unwrap();
        module.predicate_ref_by_name("IsWood").unwrap();
        module.predicate_ref_by_name("IsStick").unwrap();
        module.predicate_ref_by_name("IsWoodPick").unwrap();
        module.predicate_ref_by_name("IsStone").unwrap();
        module.predicate_ref_by_name("IsStonePick").unwrap();
    }

    #[test]
    fn test_dump_compiled_predicates() {
        let txlib_mod = Arc::new(txlib::predicates::module());
        let module = module(txlib_mod);
        for (i, predicate) in module.batch.predicates().iter().enumerate() {
            println!("{:02} {}", i, predicate.to_podlang_string());
        }
    }
}
