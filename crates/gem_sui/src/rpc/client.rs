use std::{error::Error, str::FromStr, sync::Arc};

use gem_encoding::decode_base64;
use gem_jsonrpc::grpc::GrpcTransport;
use num_bigint::BigInt;
use primitives::Chain;
use prost::Message;
use prost_types::FieldMask;
use sui_rpc::{
    Client as GrpcClient,
    field::FieldMaskUtil,
    proto::sui::rpc::v2::{
        self as proto, BatchGetObjectsRequest, BatchGetTransactionsRequest, ExecuteTransactionRequest, GetBalanceRequest, GetCheckpointRequest, GetCoinInfoRequest,
        GetEpochRequest, GetObjectRequest, GetServiceInfoRequest, GetTransactionRequest, ListBalancesRequest, ListOwnedObjectsRequest, SimulateTransactionRequest,
        Transaction as GrpcTransaction, UserSignature as GrpcUserSignature, get_object_result, get_transaction_result, simulate_transaction_request::TransactionChecks,
    },
};
use sui_types::Address;

use super::codec::{decode_grpc_message, encode_grpc_message};
use super::mapper::{map_checkpoint, map_executed_transaction, map_inspect_result, map_sui_effects};
use crate::models::transaction::{SuiBroadcastTransaction, SuiTransaction};
use crate::models::{Balance, Checkpoint, CoinAsset, Digest, InspectResult, SuiCoin, SuiCoinMetadata, SuiObject, TransactionBlocks};
use crate::{SUI_COIN_TYPE, SUI_COIN_TYPE_FULL};

const TRANSACTION_READ_MASK: &[&str] = &[
    "digest",
    "effects.gas_used",
    "effects.status",
    "effects.gas_object",
    "events.events.package_id",
    "events.events.event_type",
    "events.events.json",
    "balance_changes",
    "timestamp",
];

pub(super) const PATH_GET_EPOCH: &str = "/sui.rpc.v2.LedgerService/GetEpoch";
const PATH_GET_SERVICE_INFO: &str = "/sui.rpc.v2.LedgerService/GetServiceInfo";
const PATH_GET_OBJECT: &str = "/sui.rpc.v2.LedgerService/GetObject";
const PATH_GET_CHECKPOINT: &str = "/sui.rpc.v2.LedgerService/GetCheckpoint";
const PATH_GET_TRANSACTION: &str = "/sui.rpc.v2.LedgerService/GetTransaction";
pub(super) const PATH_LIST_OWNED_OBJECTS: &str = "/sui.rpc.v2.StateService/ListOwnedObjects";
const PATH_GET_BALANCE: &str = "/sui.rpc.v2.StateService/GetBalance";
const PATH_LIST_BALANCES: &str = "/sui.rpc.v2.StateService/ListBalances";
const PATH_GET_COIN_INFO: &str = "/sui.rpc.v2.StateService/GetCoinInfo";
const PATH_BATCH_GET_OBJECTS: &str = "/sui.rpc.v2.LedgerService/BatchGetObjects";
const PATH_BATCH_GET_TRANSACTIONS: &str = "/sui.rpc.v2.LedgerService/BatchGetTransactions";
pub(super) const PATH_SIMULATE_TRANSACTION: &str = "/sui.rpc.v2.TransactionExecutionService/SimulateTransaction";
pub(crate) const PATH_EXECUTE_TRANSACTION: &str = "/sui.rpc.v2.TransactionExecutionService/ExecuteTransaction";
const BATCH_GET_TRANSACTIONS_LIMIT: usize = 50;

#[derive(Clone, Debug)]
pub struct SuiClient {
    endpoint: String,
    transport: Option<Arc<dyn GrpcTransport>>,
}

impl SuiClient {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            transport: None,
        }
    }

    pub fn new_with_transport(endpoint: impl Into<String>, transport: Arc<dyn GrpcTransport>) -> Self {
        Self {
            endpoint: endpoint.into(),
            transport: Some(transport),
        }
    }

    pub(super) fn has_transport(&self) -> bool {
        self.transport.is_some()
    }

    pub(super) fn client(&self) -> Result<GrpcClient, Box<dyn Error + Send + Sync>> {
        Ok(GrpcClient::new(self.endpoint.clone())?)
    }

    pub(super) async fn grpc_unary<Req, Resp>(&self, path: &str, request: Req) -> Result<Resp, Box<dyn Error + Send + Sync>>
    where
        Req: Message,
        Resp: Message + Default,
    {
        let transport = self.transport.as_ref().ok_or("missing Sui gRPC transport")?;
        let body = encode_grpc_message(&request)?;
        let response = transport.unary(&self.endpoint, path, body).await?;
        decode_grpc_message(&response)
    }

    pub async fn inspect_transaction_block(&self, sender: &str, tx_data: &[u8], _gas_price: Option<u64>) -> Result<InspectResult, Box<dyn Error + Send + Sync>> {
        let transaction = decode_inspect_transaction_bytes(sender, tx_data)?;
        let request = SimulateTransactionRequest::new(transaction)
            .with_read_mask(FieldMask::from_paths([
                "transaction.effects.gas_used",
                "transaction.effects.status",
                "command_outputs.return_values.value",
            ]))
            .with_checks(TransactionChecks::Disabled);
        if self.has_transport() {
            return Ok(map_inspect_result(self.grpc_unary(PATH_SIMULATE_TRANSACTION, request).await?));
        }
        let mut client = self.client()?;
        let response = client.execution_client().simulate_transaction(request).await?.into_inner();
        Ok(map_inspect_result(response))
    }

    pub async fn get_balance(&self, address: String) -> Result<Balance, Box<dyn Error + Send + Sync>> {
        let mut request = GetBalanceRequest::default();
        request.owner = Some(address);
        request.coin_type = Some(SUI_COIN_TYPE_FULL.to_string());
        let response = if self.has_transport() {
            self.grpc_unary(PATH_GET_BALANCE, request).await?
        } else {
            let mut client = self.client()?;
            client.state_client().get_balance(request).await?.into_inner()
        };

        let balance = response.balance.and_then(|balance| balance.balance).unwrap_or_default();
        Ok(Balance {
            coin_type: SUI_COIN_TYPE_FULL.to_string(),
            total_balance: BigInt::from(balance),
        })
    }

    pub async fn get_all_balances(&self, address: String) -> Result<Vec<Balance>, Box<dyn Error + Send + Sync>> {
        let mut request = ListBalancesRequest::default();
        request.owner = Some(address);
        request.page_size = Some(1000);
        let mut balances = Vec::new();
        let mut client = if self.has_transport() { None } else { Some(self.client()?) };

        loop {
            let response = if self.has_transport() {
                self.grpc_unary(PATH_LIST_BALANCES, request.clone()).await?
            } else {
                client
                    .as_mut()
                    .ok_or("missing Sui gRPC client")?
                    .state_client()
                    .list_balances(request.clone())
                    .await?
                    .into_inner()
            };
            let page = response
                .balances
                .into_iter()
                .map(|balance| {
                    Ok(Balance {
                        coin_type: balance.coin_type.ok_or("missing Sui balance coin type")?,
                        total_balance: BigInt::from(balance.balance.ok_or("missing Sui balance amount")?),
                    })
                })
                .collect::<Result<Vec<_>, Box<dyn Error + Send + Sync>>>()?;
            balances.extend(page);
            if response.next_page_token.is_none() {
                break;
            }
            request.page_token = response.next_page_token;
        }

        Ok(balances)
    }

    pub async fn get_coin_metadata(&self, token_id: String) -> Result<SuiCoinMetadata, Box<dyn Error + Send + Sync>> {
        let mut request = GetCoinInfoRequest::default();
        request.coin_type = Some(token_id);
        let response = if self.has_transport() {
            self.grpc_unary(PATH_GET_COIN_INFO, request).await?
        } else {
            let mut client = self.client()?;
            client.state_client().get_coin_info(request).await?.into_inner()
        };
        let metadata = response.metadata.ok_or("missing Sui coin metadata")?;
        Ok(SuiCoinMetadata {
            decimals: metadata.decimals.unwrap_or_default() as i32,
            name: metadata.name.unwrap_or_default(),
            symbol: metadata.symbol.unwrap_or_default(),
        })
    }

    pub async fn get_chain_id(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        map_service_chain_identifier(self.service_info().await?)
    }

    pub async fn get_latest_block(&self) -> Result<u64, Box<dyn Error + Send + Sync>> {
        Ok(self.service_info().await?.checkpoint_height.ok_or("missing Sui checkpoint height")?)
    }

    pub async fn get_gas_price(&self) -> Result<BigInt, Box<dyn Error + Send + Sync>> {
        if self.has_transport() {
            let mut request = GetEpochRequest::latest();
            request.read_mask = Some(FieldMask::from_str("reference_gas_price"));
            let response: proto::GetEpochResponse = self.grpc_unary(PATH_GET_EPOCH, request).await?;
            let epoch = response.epoch.ok_or("missing Sui epoch")?;
            return Ok(BigInt::from(epoch.reference_gas_price.ok_or("missing Sui reference gas price")?));
        }
        let mut client = self.client()?;
        let gas_price = client.get_reference_gas_price().await?;
        Ok(BigInt::from(gas_price))
    }

    pub async fn get_coins(&self, address: &str, coin_type: &str) -> Result<Vec<SuiCoin>, Box<dyn Error + Send + Sync>> {
        let mut request = ListOwnedObjectsRequest::default()
            .with_owner(address)
            .with_page_size(1000)
            .with_object_type(format!("0x2::coin::Coin<{}>", full_sui_coin_type(coin_type)))
            .with_read_mask(FieldMask::from_paths(["object_id", "version", "digest", "balance", "object_type"]));
        let mut coins = Vec::new();
        let mut client = if self.has_transport() { None } else { Some(self.client()?) };

        loop {
            let response = if self.has_transport() {
                self.grpc_unary(PATH_LIST_OWNED_OBJECTS, request.clone()).await?
            } else {
                client
                    .as_mut()
                    .ok_or("missing Sui gRPC client")?
                    .state_client()
                    .list_owned_objects(request.clone())
                    .await?
                    .into_inner()
            };
            let page = response
                .objects
                .into_iter()
                .map(|object| {
                    Ok(SuiCoin {
                        coin_type: object
                            .object_type
                            .ok_or("missing Sui coin object type")?
                            .trim_start_matches("0x2::coin::Coin<")
                            .trim_end_matches('>')
                            .to_string(),
                        coin_object_id: object.object_id.ok_or("missing Sui coin object id")?,
                        balance: BigInt::from(object.balance.ok_or("missing Sui coin balance")?),
                        version: object.version.ok_or("missing Sui coin version")?.to_string(),
                        digest: object.digest.ok_or("missing Sui coin digest")?,
                    })
                })
                .collect::<Result<Vec<_>, Box<dyn Error + Send + Sync>>>()?;
            coins.extend(page);
            if response.next_page_token.is_none() {
                break;
            }
            request.page_token = response.next_page_token;
        }

        Ok(coins)
    }

    pub async fn get_coin_assets_by_type(&self, address: &str, coin_type: &str) -> Result<Vec<CoinAsset>, Box<dyn Error + Send + Sync>> {
        self.get_coins(address, coin_type).await?.into_iter().map(CoinAsset::try_from).collect()
    }

    pub async fn get_object(&self, object_id: String) -> Result<SuiObject, Box<dyn Error + Send + Sync>> {
        let object_id = Address::from_str(&object_id)?;
        let request = GetObjectRequest::new(&object_id).with_read_mask(FieldMask::from_paths(["object_id", "version", "digest"]));
        let response: proto::GetObjectResponse = if self.has_transport() {
            self.grpc_unary(PATH_GET_OBJECT, request).await?
        } else {
            let mut client = self.client()?;
            client.ledger_client().get_object(request).await?.into_inner()
        };
        let object = response.object.ok_or("missing Sui object")?;
        Ok(SuiObject {
            object_id: object.object_id.ok_or("missing Sui object id")?,
            digest: object.digest.ok_or("missing Sui object digest")?,
            version: object.version.ok_or("missing Sui object version")?.to_string(),
        })
    }

    pub async fn dry_run(&self, tx_data: String) -> Result<SuiTransaction, Box<dyn Error + Send + Sync>> {
        let transaction = decode_transaction_base64(&tx_data)?;
        let request = SimulateTransactionRequest::new(transaction)
            .with_read_mask(FieldMask::from_paths(["transaction.effects.gas_used", "transaction.effects.status"]))
            .with_checks(TransactionChecks::Enabled);
        let response = if self.has_transport() {
            self.grpc_unary(PATH_SIMULATE_TRANSACTION, request).await?
        } else {
            let mut client = self.client()?;
            client.execution_client().simulate_transaction(request).await?.into_inner()
        };
        let executed = response.transaction.ok_or("missing simulated transaction")?;
        Ok(SuiTransaction {
            effects: map_sui_effects(executed.effects.as_ref()),
        })
    }

    pub async fn get_transaction(&self, transaction_id: String) -> Result<Digest, Box<dyn Error + Send + Sync>> {
        let mut request = GetTransactionRequest::default();
        request.digest = Some(transaction_id);
        request.read_mask = Some(FieldMask::from_paths(TRANSACTION_READ_MASK.iter().copied()));
        let response = if self.has_transport() {
            self.grpc_unary(PATH_GET_TRANSACTION, request).await?
        } else {
            let mut client = self.client()?;
            client.ledger_client().get_transaction(request).await?.into_inner()
        };
        map_executed_transaction(response.transaction.ok_or("missing Sui transaction")?)
    }

    pub async fn get_transactions_by_block(&self, checkpoint: u64) -> Result<Checkpoint, Box<dyn Error + Send + Sync>> {
        let request = GetCheckpointRequest::by_sequence_number(checkpoint).with_read_mask(FieldMask::from_paths([
            "sequence_number",
            "digest",
            "summary",
            "contents.transactions.transaction",
        ]));
        let response: proto::GetCheckpointResponse = if self.has_transport() {
            self.grpc_unary(PATH_GET_CHECKPOINT, request).await?
        } else {
            let mut client = self.client()?;
            client.ledger_client().get_checkpoint(request).await?.into_inner()
        };
        let checkpoint = response.checkpoint.ok_or("missing Sui checkpoint")?;
        map_checkpoint(checkpoint)
    }

    pub async fn get_checkpoint_transactions(&self, checkpoint: u64, limit: Option<usize>) -> Result<TransactionBlocks, Box<dyn Error + Send + Sync>> {
        let checkpoint = self.get_transactions_by_block(checkpoint).await?;
        let digests: Vec<_> = checkpoint.transactions.into_iter().take(limit.unwrap_or(usize::MAX)).collect();
        if digests.is_empty() {
            return Ok(TransactionBlocks { data: Vec::new() });
        }

        let mut data = Vec::new();
        for digests in digests.chunks(BATCH_GET_TRANSACTIONS_LIMIT) {
            data.extend(self.batch_get_transactions(digests.to_vec()).await?);
        }
        Ok(TransactionBlocks { data })
    }

    async fn batch_get_transactions(&self, digests: Vec<String>) -> Result<Vec<Digest>, Box<dyn Error + Send + Sync>> {
        let mut request = BatchGetTransactionsRequest::default();
        request.digests = digests;
        request.read_mask = Some(FieldMask::from_paths(TRANSACTION_READ_MASK.iter().copied()));
        let response = if self.has_transport() {
            self.grpc_unary(PATH_BATCH_GET_TRANSACTIONS, request).await?
        } else {
            let mut client = self.client()?;
            client.ledger_client().batch_get_transactions(request).await?.into_inner()
        };
        response
            .transactions
            .into_iter()
            .map(|result| match result.result {
                Some(get_transaction_result::Result::Transaction(transaction)) => map_executed_transaction(transaction),
                Some(get_transaction_result::Result::Error(status)) => Err(format!("Sui transaction gRPC error {}: {}", status.code, status.message).into()),
                Some(_) => Err("unsupported Sui transaction result".into()),
                None => Err("missing Sui transaction result".into()),
            })
            .collect::<Result<Vec<_>, _>>()
    }

    pub async fn broadcast(&self, data: String, signature: String) -> Result<SuiBroadcastTransaction, Box<dyn Error + Send + Sync>> {
        let transaction = decode_transaction_base64(&data)?;
        let signature = sui_types::UserSignature::from_base64(&signature)?;
        let mut request = ExecuteTransactionRequest::default();
        request.transaction = Some(transaction);
        request.signatures = vec![GrpcUserSignature::from(signature)];
        request.read_mask = Some(FieldMask::from_paths(["digest", "effects.status"]));
        let response = if self.has_transport() {
            self.grpc_unary(PATH_EXECUTE_TRANSACTION, request).await?
        } else {
            let mut client = self.client()?;
            client.execution_client().execute_transaction(request).await?.into_inner()
        };
        Ok(SuiBroadcastTransaction {
            digest: response
                .transaction
                .and_then(|transaction| transaction.digest)
                .ok_or("missing Sui broadcast transaction digest")?,
        })
    }

    pub async fn get_multiple_objects(&self, object_ids: Vec<String>) -> Result<Vec<proto::Object>, Box<dyn Error + Send + Sync>> {
        let requests = object_ids
            .into_iter()
            .map(|object_id| Ok(GetObjectRequest::new(&Address::from_str(&object_id)?)))
            .collect::<Result<Vec<_>, Box<dyn Error + Send + Sync>>>()?;
        let mut request = BatchGetObjectsRequest::default();
        request.requests = requests;
        request.read_mask = Some(FieldMask::from_paths(["object_id", "owner"]));
        let response = if self.has_transport() {
            self.grpc_unary(PATH_BATCH_GET_OBJECTS, request).await?
        } else {
            let mut client = self.client()?;
            client.ledger_client().batch_get_objects(request).await?.into_inner()
        };
        response
            .objects
            .into_iter()
            .map(|result| match result.result {
                Some(get_object_result::Result::Object(object)) => Ok(object),
                Some(get_object_result::Result::Error(status)) => Err(format!("Sui object gRPC error {}: {}", status.code, status.message).into()),
                Some(_) => Err("unsupported Sui object result".into()),
                None => Err("missing Sui object result".into()),
            })
            .collect()
    }

    async fn service_info(&self) -> Result<proto::GetServiceInfoResponse, Box<dyn Error + Send + Sync>> {
        if self.has_transport() {
            return self.grpc_unary(PATH_GET_SERVICE_INFO, GetServiceInfoRequest::default()).await;
        }
        let mut client = self.client()?;
        Ok(client.ledger_client().get_service_info(GetServiceInfoRequest::default()).await?.into_inner())
    }

    pub(super) async fn get_epoch(&self, read_mask: Option<String>) -> Result<proto::Epoch, Box<dyn Error + Send + Sync>> {
        let mut request = GetEpochRequest::latest();
        request.read_mask = read_mask.as_deref().map(FieldMask::from_str);
        let response: proto::GetEpochResponse = if self.has_transport() {
            self.grpc_unary(PATH_GET_EPOCH, request).await?
        } else {
            let mut client = self.client()?;
            client.ledger_client().get_epoch(request).await?.into_inner()
        };
        Ok(response.epoch.ok_or("missing Sui epoch")?)
    }
}

fn decode_transaction_base64(tx_data: &str) -> Result<GrpcTransaction, Box<dyn Error + Send + Sync>> {
    decode_transaction_bytes(&decode_base64(tx_data)?)
}

fn decode_transaction_bytes(tx_data: &[u8]) -> Result<GrpcTransaction, Box<dyn Error + Send + Sync>> {
    let transaction: sui_types::Transaction = bcs::from_bytes(tx_data)?;
    Ok(GrpcTransaction::from(transaction))
}

fn decode_inspect_transaction_bytes(sender: &str, tx_data: &[u8]) -> Result<GrpcTransaction, Box<dyn Error + Send + Sync>> {
    if let Ok(transaction) = decode_transaction_bytes(tx_data) {
        return Ok(transaction);
    }
    let kind: sui_types::TransactionKind = bcs::from_bytes(tx_data)?;
    let mut transaction = GrpcTransaction::default();
    transaction.kind = Some(proto::TransactionKind::from(kind));
    transaction.sender = Some(sender.to_string());
    Ok(transaction)
}

fn full_sui_coin_type(coin_type: &str) -> String {
    match coin_type {
        SUI_COIN_TYPE => SUI_COIN_TYPE_FULL.to_string(),
        _ => coin_type.to_string(),
    }
}

fn map_service_chain_identifier(service_info: proto::GetServiceInfoResponse) -> Result<String, Box<dyn Error + Send + Sync>> {
    if service_info.chain.as_deref() == Some("mainnet") {
        return Ok(Chain::Sui.network_id().to_string());
    }

    Ok(service_info.chain_id.ok_or("missing Sui chain id")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_service_chain_identifier() {
        let mut service_info = proto::GetServiceInfoResponse::default();
        service_info.chain = Some("mainnet".to_string());
        service_info.chain_id = Some("grpc-mainnet-genesis-digest".to_string());
        assert_eq!(map_service_chain_identifier(service_info).unwrap(), Chain::Sui.network_id());

        let testnet = "sui-testnet-chain-id";
        let mut service_info = proto::GetServiceInfoResponse::default();
        service_info.chain = Some("testnet".to_string());
        service_info.chain_id = Some(testnet.to_string());
        assert_eq!(map_service_chain_identifier(service_info).unwrap(), testnet);
    }
}
