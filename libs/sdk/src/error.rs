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

impl From<Box<EvalAltResult>> for SdkError {
    fn from(e: Box<EvalAltResult>) -> Self {
        let position = e.position();
        let msg = format!("{e}");
        Self::Eval(msg, position)
    }
}
