use thiserror::Error;

#[derive(Debug, Error)]
pub enum InnerError {
    #[error("inner error")]
    Inner,
}

#[derive(Debug, Error)]
pub enum OuterError {
    #[error(transparent)]
    Inner(#[from] Box<InnerError>),
}

fn main() {
    let e = InnerError::Inner;
    let be = Box::new(e);
    let _ = OuterError::from(be);
}
