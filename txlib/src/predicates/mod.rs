use pod2::lang::{self, load_module};

pub fn module() -> lang::Module {
    let params = pod2::middleware::Params::default();
    load_module(include_str!("txlib.podlang"), "txlib", &params, &[]).expect("compiles")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predicates_exist() {
        let module = module();
        println!("txlib id: {:#}", module.batch.id());
        module.predicate_ref_by_name("Tx").unwrap();
        module
            .predicate_ref_by_name("TxObjectStateNullified")
            .unwrap();
        module.predicate_ref_by_name("TxFinalized").unwrap();
        module.predicate_ref_by_name("InputsGrounded").unwrap();
    }
}
