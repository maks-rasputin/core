use super::{
    cache::PoolCache,
    constants::{CETUS_TICK_SPACINGS, KNOWN_POOLS},
    model::{DiscoveredPool, Hop, INTERMEDIATE_COIN_TYPES},
    tx_builder,
};
use crate::{
    ProviderType, RpcClient, RpcProvider, SwapperError, SwapperProvider,
    client_factory::create_client_with_chain,
    fees::{ReferralFee, default_referral_fees},
};
use gem_client::Client;
use gem_sui::{EMPTY_ADDRESS, SUI_COIN_TYPE, SuiClient, coin_type_matches, full_coin_type, models::InspectResult, tx_builder::ObjectResolver};
use primitives::{AssetId, Chain};
use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    sync::Arc,
};

#[derive(Debug)]
pub(super) struct QuoteResult {
    pub amount_out: u64,
    pub after_sqrt_price: u128,
    pub is_exceed: bool,
}

pub struct CetusClmm<C>
where
    C: Client + Clone + Send + Sync + Debug + 'static,
{
    pub(super) provider: ProviderType,
    pub(super) sui_client: SuiClient<C>,
    pool_cache: PoolCache,
}

impl<C: Client + Clone + Send + Sync + Debug + 'static> Debug for CetusClmm<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CetusClmm")
    }
}

impl CetusClmm<RpcClient> {
    pub fn new(rpc_provider: Arc<dyn RpcProvider>) -> Self {
        let sui_client = create_client_with_chain(rpc_provider, Chain::Sui);
        Self::with_client(SuiClient::new(sui_client))
    }
}

impl<C> CetusClmm<C>
where
    C: Client + Clone + Send + Sync + Debug + 'static,
{
    pub fn with_client(sui_client: SuiClient<C>) -> Self {
        Self {
            provider: ProviderType::new(SwapperProvider::CetusClmm),
            sui_client,
            pool_cache: PoolCache::default(),
        }
    }

    pub(super) fn referral_fee() -> ReferralFee {
        default_referral_fees().sui
    }

    pub(super) fn coin_type(asset_id: &AssetId) -> String {
        full_coin_type(asset_id.token_id.as_deref().unwrap_or(SUI_COIN_TYPE))
    }

    pub(super) async fn find_route_hops(&self, from: &str, to: &str, swap_amount: u64) -> Result<Vec<Hop>, SwapperError> {
        let mut candidates: Vec<Vec<DiscoveredPool>> = self.discover_direct_pools(from, to).await.into_iter().map(|pool| vec![pool]).collect();
        for raw_intermediate in INTERMEDIATE_COIN_TYPES {
            let intermediate = full_coin_type(raw_intermediate);
            if coin_type_matches(from, &intermediate) || coin_type_matches(to, &intermediate) {
                continue;
            }
            let (firsts, seconds) = futures::future::join(self.discover_direct_pools(from, &intermediate), self.discover_direct_pools(&intermediate, to)).await;
            for first in &firsts {
                for second in &seconds {
                    candidates.push(vec![first.clone(), second.clone()]);
                }
            }
        }
        if candidates.is_empty() {
            return Err(SwapperError::NoQuoteAvailable);
        }
        let quotes = futures::future::join_all(candidates.into_iter().map(|pools| self.quote_candidate(pools, from, swap_amount))).await;
        quotes
            .into_iter()
            .flatten()
            .max_by_key(|hops| hops.last().map(|h| h.amount_out).unwrap_or_default())
            .ok_or(SwapperError::NoQuoteAvailable)
    }

    fn known_pools(from: &str, to: &str) -> Vec<DiscoveredPool> {
        KNOWN_POOLS
            .iter()
            .filter(|known| {
                (coin_type_matches(from, known.coin_a) && coin_type_matches(to, known.coin_b)) || (coin_type_matches(from, known.coin_b) && coin_type_matches(to, known.coin_a))
            })
            .map(|known| DiscoveredPool {
                pool_id: known.pool_id.to_string(),
                pool_init_version: known.pool_init_version,
                coin_a: known.coin_a.to_string(),
                coin_b: known.coin_b.to_string(),
            })
            .collect()
    }

    async fn discover_direct_pools(&self, from: &str, to: &str) -> Vec<DiscoveredPool> {
        let known = Self::known_pools(from, to);
        if !known.is_empty() {
            return known;
        }
        if let Some(cached) = self.pool_cache.get(from, to) {
            return cached;
        }
        let Some(pools) = self.query_direct_pools(from, to).await else {
            return Vec::new();
        };
        self.pool_cache.put(from, to, &pools);
        pools
    }

    async fn query_direct_pools(&self, from: &str, to: &str) -> Option<Vec<DiscoveredPool>> {
        let (coin_a, coin_b) = canonical_pair_order(from, to);
        let inspects = CETUS_TICK_SPACINGS.iter().map(|tick| self.inspect_pool_id(coin_a, coin_b, *tick));
        let results = futures::future::join_all(inspects).await;
        let mut candidates: Vec<(String, String, String)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for result in results {
            match result {
                Ok(Some(pool_id)) => {
                    if seen.insert(pool_id.clone()) {
                        candidates.push((pool_id, coin_a.to_string(), coin_b.to_string()));
                    }
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        if candidates.is_empty() {
            return Some(Vec::new());
        }
        let pool_ids: Vec<String> = candidates.iter().map(|(id, _, _)| id.clone()).collect();
        let resolver = ObjectResolver::prefetch(&self.sui_client, pool_ids, &HashMap::new()).await.ok()?;
        Some(
            candidates
                .into_iter()
                .filter_map(|(pool_id, coin_a, coin_b)| {
                    let pool_init_version = resolver.initial_shared_version(&pool_id)?;
                    Some(DiscoveredPool {
                        pool_id,
                        pool_init_version,
                        coin_a,
                        coin_b,
                    })
                })
                .collect(),
        )
    }

    async fn quote_candidate(&self, pools: Vec<DiscoveredPool>, from: &str, swap_amount: u64) -> Option<Vec<Hop>> {
        let hop_count = pools.len();
        let mut hops: Vec<Hop> = Vec::with_capacity(hop_count);
        let mut current_coin = from.to_string();
        let mut current_amount = swap_amount;
        for (idx, pool) in pools.into_iter().enumerate() {
            let mut hop = pool.into_hop(&current_coin, current_amount);
            let quote = self.inspect_swap_quote(&hop, current_amount).await.ok()?;
            if quote.amount_out == 0 || quote.is_exceed {
                return None;
            }
            hop.amount_out = quote.amount_out;
            hop.after_sqrt_price = quote.after_sqrt_price;
            if idx + 1 < hop_count {
                current_amount = quote.amount_out;
                current_coin = hop.output_coin_type().to_string();
            }
            hops.push(hop);
        }
        Some(hops)
    }

    async fn inspect_pool_id(&self, coin_a: &str, coin_b: &str, tick_spacing: u32) -> Result<Option<String>, SwapperError> {
        let transaction = tx_builder::build_pool_id_inspect(coin_a, coin_b, tick_spacing)?;
        let result = self
            .sui_client
            .inspect_transaction_block(EMPTY_ADDRESS, &transaction, None)
            .await
            .map_err(|err| SwapperError::ComputeQuoteError(err.to_string()))?;
        if result.error.is_some() {
            return Ok(None);
        }
        let bytes = result
            .results
            .last()
            .and_then(|command| command.return_values.first())
            .map(|(bytes, _)| bytes)
            .ok_or_else(|| SwapperError::ComputeQuoteError("Cetus CLMM pool discovery returned no value".into()))?;
        if bytes.len() != 32 {
            return Err(SwapperError::ComputeQuoteError("Cetus CLMM pool discovery returned invalid id".into()));
        }
        Ok(Some(format!("0x{}", hex::encode(bytes))))
    }

    async fn inspect_swap_quote(&self, hop: &Hop, amount_in: u64) -> Result<QuoteResult, SwapperError> {
        let transaction = tx_builder::build_quote_inspect(hop, amount_in)?;
        let result = self
            .sui_client
            .inspect_transaction_block(EMPTY_ADDRESS, &transaction, None)
            .await
            .map_err(|err| SwapperError::ComputeQuoteError(err.to_string()))?;
        decode_quote_result(&result)
    }
}

fn canonical_pair_order<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a > b { (a, b) } else { (b, a) }
}

fn decode_quote_result(result: &InspectResult) -> Result<QuoteResult, SwapperError> {
    if result.error.is_some() {
        return Err(SwapperError::NoQuoteAvailable);
    }
    let bytes = result
        .results
        .first()
        .and_then(|command| command.return_values.first())
        .map(|(bytes, _)| bytes)
        .ok_or_else(|| SwapperError::ComputeQuoteError("Cetus CLMM quote inspect returned no value".into()))?;
    if bytes.len() < 49 {
        return Err(SwapperError::ComputeQuoteError("Cetus CLMM quote inspect returned truncated CalculatedSwapResult".into()));
    }
    let amount_out = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| SwapperError::ComputeQuoteError("Cetus CLMM amount_out decode failed".into()))?,
    );
    let after_sqrt_price = u128::from_le_bytes(
        bytes[32..48]
            .try_into()
            .map_err(|_| SwapperError::ComputeQuoteError("Cetus CLMM after_sqrt_price decode failed".into()))?,
    );
    let is_exceed = bytes[48] != 0;
    Ok(QuoteResult {
        amount_out,
        after_sqrt_price,
        is_exceed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspect_result(bytes: Vec<u8>) -> InspectResult {
        InspectResult {
            effects: gem_sui::models::InspectEffects {
                gas_used: gem_sui::models::InspectGasUsed {
                    computation_cost: 0,
                    storage_cost: 0,
                    storage_rebate: 0,
                },
            },
            events: serde_json::Value::Null,
            error: None,
            results: vec![gem_sui::models::InspectCommandResult {
                return_values: vec![(bytes, "CalculatedSwapResult".into())],
            }],
        }
    }

    fn calc_swap_bytes(amount_out: u64, current_sqrt: u128, after_sqrt: u128, is_exceed: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(66);
        bytes.extend_from_slice(&997_500_u64.to_le_bytes());
        bytes.extend_from_slice(&amount_out.to_le_bytes());
        bytes.extend_from_slice(&2_500_u64.to_le_bytes());
        bytes.extend_from_slice(&2_500_u64.to_le_bytes());
        bytes.extend_from_slice(&after_sqrt.to_le_bytes());
        bytes.push(if is_exceed { 1 } else { 0 });
        bytes.push(1);
        bytes.extend_from_slice(&current_sqrt.to_le_bytes());
        bytes
    }

    #[test]
    fn test_canonical_pair_order() {
        let sui = gem_sui::SUI_COIN_TYPE_FULL;
        let usdc = "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC";
        let blue = "0xe1b45a0e641b9955a20aa0ad1c1f4ad86aad8afb07296d4085e349a50e90bdca::blue::BLUE";

        assert_eq!(canonical_pair_order(usdc, sui), (usdc, sui));
        assert_eq!(canonical_pair_order(sui, usdc), (usdc, sui));
        assert_eq!(canonical_pair_order(blue, sui), (blue, sui));
        assert_eq!(canonical_pair_order(sui, blue), (blue, sui));
        assert_eq!(canonical_pair_order(blue, usdc), (blue, usdc));
        assert_eq!(canonical_pair_order(usdc, blue), (blue, usdc));
    }

    #[test]
    fn test_decode_quote_result() {
        let current = 521_723_622_374_070_550_528_u128;
        let after = 521_460_761_563_383_315_264_u128;
        let bytes = calc_swap_bytes(796_985_864, current, after, false);
        let decoded = decode_quote_result(&inspect_result(bytes)).unwrap();
        assert_eq!(decoded.amount_out, 796_985_864);
        assert_eq!(decoded.after_sqrt_price, after);
        assert!(!decoded.is_exceed);

        let exceeded = calc_swap_bytes(796_985_864, current, after, true);
        assert!(decode_quote_result(&inspect_result(exceeded)).unwrap().is_exceed);

        let truncated = decode_quote_result(&inspect_result(vec![0u8; 16]));
        match truncated {
            Err(SwapperError::ComputeQuoteError(_)) => {}
            other => panic!("expected ComputeQuoteError, got {other:?}"),
        }
    }
}
