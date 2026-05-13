use crate::SwapperError;
use std::fmt::Display;

pub(super) fn error(err: impl Display) -> SwapperError {
    SwapperError::TransactionError(err.to_string())
}
