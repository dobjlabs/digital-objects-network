use std::rc::Rc;

use rhai::{EvalAltResult, Position};

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("eval at {1}: {0}")]
    Eval(String, Position),
    #[error("anyhow: {0}")]
    Anyhow(anyhow::Error),
}

impl From<anyhow::Error> for SdkError {
    fn from(e: anyhow::Error) -> Self {
        Self::Anyhow(e)
    }
}

fn innermost(e: &EvalAltResult) -> &EvalAltResult {
    match e {
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _) => innermost(inner),
        other => other,
    }
}

impl From<Box<EvalAltResult>> for SdkError {
    fn from(e: Box<EvalAltResult>) -> Self {
        // Unwrap the runtime error because otherwise the Display impl only formats the type and
        // not the value.
        if let EvalAltResult::ErrorRuntime(payload, pos) = innermost(&e)
            && let Some(e) = payload.read_lock::<Rc<anyhow::Error>>()
        {
            return Self::Eval(format!("{}", &*e), *pos);
        }
        Self::Eval(format!("{e}"), e.position())
    }
}
