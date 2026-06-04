use pod2::{
    backends::plonky2::Result,
    middleware::{
        self, EMPTY_HASH, Hash, IntroPredicateRef, Params, Pod, Proof, Statement, VDSet, Value,
        VerifierOnlyCircuitData, containers::Array,
    },
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockIntroPod {
    params: Params,
    vd_set: VDSet,
    statement: Statement,
    vd_hash: Hash,
}

fn intro_self_statement(name: String, args: Vec<Value>) -> Statement {
    Statement::Intro(
        IntroPredicateRef {
            name,
            args_len: args.len(),
            verifier_data_hash: EMPTY_HASH,
        },
        args.into_iter().map(Into::into).collect(),
    )
}

impl MockIntroPod {
    /// Create a new `MockIntroPod` that has a public statement with an intro predicate named
    /// `name` and identified by verifier data hash `vd_hash` with arguments `args`.
    pub fn new(
        params: &Params,
        vd_set: VDSet,
        name: String,
        vd_hash: Hash,
        args: Vec<Value>,
    ) -> Self {
        let statement = intro_self_statement(name, args);
        Self {
            params: params.clone(),
            vd_set,
            statement,
            vd_hash,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    vd_hash: Hash,
    statement: Statement,
}

impl Pod for MockIntroPod {
    fn params(&self) -> &Params {
        &self.params
    }
    fn is_mock(&self) -> bool {
        true
    }
    fn is_main(&self) -> bool {
        false
    }

    fn verify(&self) -> Result<()> {
        log::warn!("MockIntroPod doesn't verify anything");
        Ok(())
    }

    fn pod_type(&self) -> (usize, &'static str) {
        (999, "MockIntro")
    }

    fn pub_raw_statements_mt(&self) -> Array {
        Array::new(vec![Value::from(self.statement.hash())])
    }

    fn pub_raw_statements(&self) -> Vec<middleware::Statement> {
        vec![self.statement.clone()]
    }

    fn verifier_data_hash(&self) -> Hash {
        self.vd_hash
    }
    fn verifier_data(&self) -> VerifierOnlyCircuitData {
        panic!("MockIntroPod can't be verified in a recursive MainPod circuit");
    }
    fn common_hash(&self) -> String {
        panic!("MockIntroPod can't be verified in a recursive MainPod circuit");
    }
    fn proof(&self) -> Proof {
        panic!("MockIntroPod can't be verified in a recursive MainPod circuit");
    }
    fn vd_set(&self) -> &VDSet {
        &self.vd_set
    }

    fn serialize_data(&self) -> serde_json::Value {
        serde_json::to_value(Data {
            statement: self.statement.clone(),
            vd_hash: self.vd_hash,
        })
        .expect("serialization to json")
    }
    fn deserialize_data(params: Params, data: serde_json::Value, vd_set: VDSet) -> Result<Self> {
        let data: Data = serde_json::from_value(data)?;
        Ok(Self {
            params,
            vd_set,
            statement: data.statement,
            vd_hash: data.vd_hash,
        })
    }
}
