use std::{error::Error, str::FromStr};

use num_bigint::{BigInt, BigUint};
use prost_types::{ListValue, Struct, Value, value::Kind};
use sui_rpc::proto::sui::rpc::v2::{self as proto, owner::OwnerKind};

use crate::models::transaction::SuiStatus;
use crate::models::{
    BalanceChange, Checkpoint, Digest, Effect, Event, GasObject, GasUsed, InspectCommandResult, InspectEffects, InspectGasUsed, InspectResult, Owner, OwnerObject, Status,
    SuiEffects,
};

pub(super) fn timestamp_millis(timestamp: &prost_types::Timestamp) -> i64 {
    timestamp.seconds.saturating_mul(1000) + i64::from(timestamp.nanos / 1_000_000)
}

pub(super) fn map_checkpoint(checkpoint: proto::Checkpoint) -> Result<Checkpoint, Box<dyn Error + Send + Sync>> {
    let summary = required(checkpoint.summary, "missing Sui checkpoint summary")?;
    let contents = required(checkpoint.contents, "missing Sui checkpoint contents")?;
    let transactions = contents
        .transactions
        .into_iter()
        .map(|transaction| required(transaction.transaction, "missing Sui checkpoint transaction digest"))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Checkpoint {
        epoch: required(summary.epoch, "missing Sui checkpoint epoch")?.to_string(),
        sequence_number: required(checkpoint.sequence_number, "missing Sui checkpoint sequence number")?.to_string(),
        digest: required(checkpoint.digest, "missing Sui checkpoint digest")?,
        network_total_transactions: required(summary.total_network_transactions, "missing Sui checkpoint network transaction count")?.to_string(),
        previous_digest: summary.previous_digest.unwrap_or_default(),
        timestamp_ms: timestamp_millis(&required(summary.timestamp, "missing Sui checkpoint timestamp")?).to_string(),
        transactions,
    })
}

pub(super) fn map_executed_transaction(transaction: proto::ExecutedTransaction) -> Result<Digest, Box<dyn Error + Send + Sync>> {
    Ok(Digest {
        digest: transaction.digest.ok_or("missing Sui transaction digest")?,
        effects: map_effect(transaction.effects.as_ref()),
        balance_changes: Some(transaction.balance_changes.into_iter().map(map_balance_change).collect::<Result<Vec<_>, _>>()?),
        events: transaction.events.map(map_events).unwrap_or_default(),
        timestamp_ms: transaction.timestamp.as_ref().map(timestamp_millis).unwrap_or_default() as u64,
    })
}

fn map_effect(effects: Option<&proto::TransactionEffects>) -> Effect {
    let gas_object_owner = effects
        .and_then(|effects| effects.gas_object.as_ref())
        .and_then(|object| object.output_owner.as_ref().or(object.input_owner.as_ref()))
        .map(map_owner)
        .unwrap_or_else(|| Owner::String(String::new()));

    Effect {
        gas_used: map_gas_used(effects.and_then(|effects| effects.gas_used.as_ref())),
        status: Status {
            status: if transaction_success(effects) { "success" } else { "failure" }.to_string(),
        },
        gas_object: GasObject { owner: gas_object_owner },
    }
}

pub(super) fn map_sui_effects(effects: Option<&proto::TransactionEffects>) -> SuiEffects {
    SuiEffects {
        gas_used: map_gas_used(effects.and_then(|effects| effects.gas_used.as_ref())),
        status: SuiStatus {
            status: if transaction_success(effects) { "success" } else { "failure" }.to_string(),
            error: transaction_error(effects),
        },
    }
}

fn transaction_success(effects: Option<&proto::TransactionEffects>) -> bool {
    effects.and_then(|effects| effects.status.as_ref()).and_then(|status| status.success).unwrap_or(false)
}

fn transaction_error(effects: Option<&proto::TransactionEffects>) -> Option<String> {
    effects
        .and_then(|effects| effects.status.as_ref())
        .and_then(|status| status.error.as_ref())
        .and_then(|error| error.description.clone())
}

fn map_gas_used(gas: Option<&proto::GasCostSummary>) -> GasUsed {
    GasUsed {
        computation_cost: BigUint::from(gas.and_then(|gas| gas.computation_cost).unwrap_or_default()),
        storage_cost: BigUint::from(gas.and_then(|gas| gas.storage_cost).unwrap_or_default()),
        storage_rebate: BigUint::from(gas.and_then(|gas| gas.storage_rebate).unwrap_or_default()),
        non_refundable_storage_fee: BigUint::from(gas.and_then(|gas| gas.non_refundable_storage_fee).unwrap_or_default()),
    }
}

fn map_balance_change(change: proto::BalanceChange) -> Result<BalanceChange, Box<dyn Error + Send + Sync>> {
    Ok(BalanceChange {
        owner: Owner::OwnerObject(OwnerObject { address_owner: change.address }),
        coin_type: change.coin_type.ok_or("missing Sui balance change coin type")?,
        amount: BigInt::from_str(&change.amount.ok_or("missing Sui balance change amount")?)?,
    })
}

fn map_events(events: proto::TransactionEvents) -> Vec<Event> {
    events
        .events
        .into_iter()
        .map(|event| Event {
            event_type: event.event_type.unwrap_or_default(),
            parsed_json: event.json.as_deref().map(prost_value_to_json),
            package_id: event.package_id.unwrap_or_default(),
        })
        .collect()
}

fn map_owner(owner: &proto::Owner) -> Owner {
    match OwnerKind::try_from(owner.kind.unwrap_or_default()) {
        Ok(OwnerKind::Address) => Owner::OwnerObject(OwnerObject {
            address_owner: owner.address.clone(),
        }),
        _ => Owner::String(owner.address.clone().unwrap_or_default()),
    }
}

pub(super) fn map_inspect_result(response: proto::SimulateTransactionResponse) -> InspectResult {
    let effects = response.transaction.as_ref().and_then(|transaction| transaction.effects.as_ref());
    let gas_used = effects.and_then(|effects| effects.gas_used.as_ref());

    InspectResult {
        effects: InspectEffects {
            gas_used: InspectGasUsed {
                computation_cost: gas_used.and_then(|gas| gas.computation_cost).unwrap_or_default(),
                storage_cost: gas_used.and_then(|gas| gas.storage_cost).unwrap_or_default(),
                storage_rebate: gas_used.and_then(|gas| gas.storage_rebate).unwrap_or_default(),
            },
        },
        events: serde_json::Value::Null,
        error: transaction_error(effects),
        results: response
            .command_outputs
            .into_iter()
            .map(|output| InspectCommandResult {
                return_values: output
                    .return_values
                    .into_iter()
                    .filter_map(|value| {
                        let value = value.value?;
                        Some((value.value.unwrap_or_default().to_vec(), value.name.unwrap_or_default()))
                    })
                    .collect(),
            })
            .collect(),
    }
}

fn prost_value_to_json(value: &Value) -> serde_json::Value {
    match value.kind.as_ref() {
        Some(Kind::NullValue(_)) | None => serde_json::Value::Null,
        Some(Kind::NumberValue(value)) => serde_json::Number::from_f64(*value).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        Some(Kind::StringValue(value)) => serde_json::Value::String(value.clone()),
        Some(Kind::BoolValue(value)) => serde_json::Value::Bool(*value),
        Some(Kind::StructValue(value)) => struct_to_json(value),
        Some(Kind::ListValue(value)) => list_to_json(value),
    }
}

fn struct_to_json(value: &Struct) -> serde_json::Value {
    serde_json::Value::Object(value.fields.iter().map(|(key, value)| (key.clone(), prost_value_to_json(value))).collect())
}

fn list_to_json(value: &ListValue) -> serde_json::Value {
    serde_json::Value::Array(value.values.iter().map(prost_value_to_json).collect())
}

fn required<T>(value: Option<T>, message: &'static str) -> Result<T, Box<dyn Error + Send + Sync>> {
    value.ok_or_else(|| message.into())
}
