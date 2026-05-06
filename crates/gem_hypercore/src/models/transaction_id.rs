use std::fmt::{Display, Formatter};

use crate::models::action::ACTION_ID_PREFIX;

const ORDER_ID_PREFIX: &str = "order:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyperCoreTransactionId {
    Order(u64),
    Action(u64),
}

impl HyperCoreTransactionId {
    pub fn parse(id: &str) -> Option<Self> {
        if let Some(value) = id.strip_prefix(ORDER_ID_PREFIX) {
            return value.parse().ok().map(Self::Order);
        }
        if let Some(value) = id.strip_prefix(ACTION_ID_PREFIX) {
            return value.parse().ok().map(Self::Action);
        }
        id.parse().ok().map(Self::Order)
    }

    pub fn value(&self) -> u64 {
        match self {
            Self::Order(value) | Self::Action(value) => *value,
        }
    }
}

impl Display for HyperCoreTransactionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Order(_) => write!(formatter, "{ORDER_ID_PREFIX}{}", self.value()),
            Self::Action(_) => write!(formatter, "{ACTION_ID_PREFIX}{}", self.value()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hypercore_transaction_id() {
        assert_eq!(HyperCoreTransactionId::parse("order:413978262893"), Some(HyperCoreTransactionId::Order(413978262893)));
        assert_eq!(HyperCoreTransactionId::parse("413978262893"), Some(HyperCoreTransactionId::Order(413978262893)));
        assert_eq!(HyperCoreTransactionId::parse("action:1778110454168"), Some(HyperCoreTransactionId::Action(1778110454168)));
        assert_eq!(HyperCoreTransactionId::parse("0xba3b"), None);

        assert_eq!(HyperCoreTransactionId::Order(413978262893).to_string(), "order:413978262893");
        assert_eq!(HyperCoreTransactionId::Action(1778110454168).to_string(), "action:1778110454168");

        assert_eq!(HyperCoreTransactionId::Order(413978262893).value(), 413978262893);
        assert_eq!(HyperCoreTransactionId::Action(1778110454168).value(), 1778110454168);
    }
}
