use async_trait::async_trait;
use chain_traits::ChainTransactionState;
use primitives::{TransactionStateRequest, TransactionUpdate};
use std::error::Error;

use gem_client::Client;

use crate::{models::transaction_id::HyperCoreTransactionId, provider::transaction_state_mapper, rpc::client::HyperCoreClient};

#[async_trait]
impl<C: Client> ChainTransactionState for HyperCoreClient<C> {
    async fn get_transaction_status(&self, request: TransactionStateRequest) -> Result<TransactionUpdate, Box<dyn Error + Sync + Send>> {
        self.transaction_state(request).await
    }
}

impl<C: Client> HyperCoreClient<C> {
    pub async fn transaction_state(&self, request: TransactionStateRequest) -> Result<TransactionUpdate, Box<dyn Error + Sync + Send>> {
        let id = HyperCoreTransactionId::parse(&request.id).ok_or("Invalid Hypercore transaction id")?;

        match id {
            HyperCoreTransactionId::Order(oid) => {
                let start_time = request.created_at - 5_000;
                let fills = self.get_user_fills_by_time(&request.sender_address, start_time).await?;
                Ok(transaction_state_mapper::map_transaction_state_order(fills, oid, request.id))
            }
            HyperCoreTransactionId::Action(nonce) => self.action_state(&request, nonce).await,
        }
    }

    async fn action_state(&self, request: &TransactionStateRequest, nonce: u64) -> Result<TransactionUpdate, Box<dyn Error + Sync + Send>> {
        let updates = self.get_ledger_updates(&request.sender_address).await?;
        Ok(transaction_state_mapper::map_transaction_state_action(updates, nonce, request.id.clone()))
    }
}
