use std::error::Error;

use crate::models::SuiBroadcastTransaction;
use crate::rpc::codec::decode_grpc_message;
use sui_rpc::proto::sui::rpc::v2::ExecuteTransactionResponse;

pub fn map_transaction_broadcast_request(data: &str) -> Result<(String, String), Box<dyn Error + Sync + Send>> {
    let parts = data.split_once('_').ok_or("Invalid transaction data format. Expected format: data_signature")?;
    Ok((parts.0.to_string(), parts.1.to_string()))
}

pub(crate) fn map_transaction_broadcast_response(response: SuiBroadcastTransaction) -> Result<String, Box<dyn Error + Sync + Send>> {
    Ok(response.digest)
}

pub fn map_transaction_broadcast_response_from_str(response: &str) -> Result<String, Box<dyn Error + Sync + Send>> {
    map_transaction_broadcast_response(serde_json::from_str::<SuiBroadcastTransaction>(response)?)
}

pub fn map_transaction_broadcast_response_from_grpc(response: &[u8]) -> Result<String, Box<dyn Error + Sync + Send>> {
    let response: ExecuteTransactionResponse = decode_grpc_message(response)?;
    map_transaction_broadcast_response(SuiBroadcastTransaction {
        digest: response
            .transaction
            .and_then(|transaction| transaction.digest)
            .ok_or("missing Sui broadcast transaction digest")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use sui_rpc::proto::sui::rpc::v2::ExecutedTransaction;

    fn grpc_frame<M: Message>(message: &M) -> Vec<u8> {
        let mut payload = Vec::new();
        message.encode(&mut payload).unwrap();

        let mut frame = Vec::new();
        frame.push(0);
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    #[test]
    fn test_map_transaction_broadcast_response_from_grpc() {
        let digest = "HgFLiBHYjYKhEh5NPY52Zm9bhwrs4W6AxE46gMU7nwhZ";
        let mut transaction = ExecutedTransaction::default();
        transaction.digest = Some(digest.to_string());
        let mut response = ExecuteTransactionResponse::default();
        response.transaction = Some(transaction);

        assert_eq!(map_transaction_broadcast_response_from_grpc(&grpc_frame(&response)).unwrap(), digest);
    }
}
