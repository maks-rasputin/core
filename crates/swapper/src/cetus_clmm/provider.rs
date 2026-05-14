use super::{
    cache::PoolCache,
    constants::{CETUS_CLMM_PUBLISHED_AT, CETUS_TICK_SPACINGS, KNOWN_POOLS},
    model::{DiscoveredPool, FeeSide, Hop, INTERMEDIATE_COIN_TYPES, PoolRoute},
    tx_builder,
};
use crate::{
    FetchQuoteData, ProviderData, ProviderType, Quote, QuoteRequest, Route, RpcClient, RpcProvider, Swapper, SwapperChainAsset, SwapperError, SwapperProvider, SwapperQuoteData,
    client_factory::create_client_with_chain,
    fees::{ReferralFee, default_referral_fees, quote_value_after_reserve_by_chain},
};
use async_trait::async_trait;
use gem_client::Client;
use gem_sui::{EMPTY_ADDRESS, SUI_COIN_TYPE, SuiClient, coin_type_matches, full_coin_type, models::InspectResult, tx_builder::ObjectResolver};
use primitives::{AssetId, Chain};
use std::{
    collections::{HashMap, HashSet},
    fmt::Debug,
    sync::Arc,
};

#[derive(Debug)]
struct QuoteResult {
    amount_out: u64,
    after_sqrt_price: u128,
    is_exceed: bool,
}

pub struct CetusClmm<C>
where
    C: Client + Clone + Send + Sync + Debug + 'static,
{
    provider: ProviderType,
    sui_client: SuiClient<C>,
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

    fn referral_fee() -> ReferralFee {
        default_referral_fees().sui
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
        let attempts: Vec<(u32, String, String)> = CETUS_TICK_SPACINGS
            .iter()
            .flat_map(|tick| [(from, to), (to, from)].map(|(a, b)| (*tick, a.to_string(), b.to_string())))
            .collect();
        let inspects = attempts.iter().map(|(tick, a, b)| self.inspect_pool_id(a, b, *tick));
        let results = futures::future::join_all(inspects).await;
        let mut candidates: Vec<(String, String, String)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for ((_, coin_a, coin_b), result) in attempts.into_iter().zip(results) {
            match result {
                Ok(Some(pool_id)) => {
                    if seen.insert(pool_id.clone()) {
                        candidates.push((pool_id, coin_a, coin_b));
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

    async fn first_pool(&self, from: &str, to: &str) -> Option<DiscoveredPool> {
        self.discover_direct_pools(from, to).await.into_iter().next()
    }

    async fn find_route_hops(&self, from: &str, to: &str, swap_amount: u64) -> Result<Vec<Hop>, SwapperError> {
        let mut candidates: Vec<Vec<DiscoveredPool>> = self.discover_direct_pools(from, to).await.into_iter().map(|pool| vec![pool]).collect();
        for raw_intermediate in INTERMEDIATE_COIN_TYPES {
            let intermediate = full_coin_type(raw_intermediate);
            if coin_type_matches(from, &intermediate) || coin_type_matches(to, &intermediate) {
                continue;
            }
            let (first, second) = futures::future::join(self.first_pool(from, &intermediate), self.first_pool(&intermediate, to)).await;
            if let (Some(first), Some(second)) = (first, second) {
                candidates.push(vec![first, second]);
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

    fn coin_type(asset_id: &AssetId) -> String {
        full_coin_type(asset_id.token_id.as_deref().unwrap_or(SUI_COIN_TYPE))
    }
}

#[async_trait]
impl<C> Swapper for CetusClmm<C>
where
    C: Client + Clone + Send + Sync + Debug + 'static,
{
    fn provider(&self) -> &ProviderType {
        &self.provider
    }

    fn supported_assets(&self) -> Vec<SwapperChainAsset> {
        vec![SwapperChainAsset::All(Chain::Sui)]
    }

    async fn get_quote(&self, request: &QuoteRequest) -> Result<Quote, SwapperError> {
        let from_value = quote_value_after_reserve_by_chain(request)?;
        let from_asset = request.from_asset.asset_id();
        let to_asset = request.to_asset.asset_id();
        let amount = from_value.parse::<u64>()?;
        if amount == 0 {
            return Err(SwapperError::InputAmountError { min_amount: Some("1".into()) });
        }

        let from_coin_type = Self::coin_type(&from_asset);
        let to_coin_type = Self::coin_type(&to_asset);
        let fee_side = FeeSide::select(&from_coin_type, &to_coin_type);

        let referral_fee = Self::referral_fee();
        let input_fee_amount = tx_builder::referral_fee_amount(amount, referral_fee.bps)?;
        let swap_amount = match fee_side {
            FeeSide::Input => amount
                .checked_sub(input_fee_amount)
                .ok_or_else(|| SwapperError::ComputeQuoteError("Cetus CLMM referral fee exceeds input amount".into()))?,
            FeeSide::Output => amount,
        };

        let slippage_bps = request.options.slippage.bps;
        let hops = self.find_route_hops(&from_coin_type, &to_coin_type, swap_amount).await?;
        let gross_amount_out = hops.last().map(|h| h.amount_out).unwrap_or_default();
        let fee_amount = match fee_side {
            FeeSide::Input => input_fee_amount,
            FeeSide::Output => tx_builder::referral_fee_amount(gross_amount_out, referral_fee.bps)?,
        };
        let route = PoolRoute { hops, fee_amount, fee_side };

        Ok(Quote {
            from_value,
            to_value: route.net_amount_out().to_string(),
            data: ProviderData {
                provider: self.provider().clone(),
                routes: vec![Route {
                    input: from_asset,
                    output: to_asset,
                    route_data: serde_json::to_string(&route)?,
                }],
                slippage_bps,
            },
            request: request.clone(),
            eta_in_seconds: Some(0),
        })
    }

    async fn get_quote_data(&self, quote: &Quote, _data: FetchQuoteData) -> Result<SwapperQuoteData, SwapperError> {
        let route_entry = quote.data.routes.first().ok_or(SwapperError::InvalidRoute)?;
        let route: PoolRoute = serde_json::from_str(&route_entry.route_data).map_err(|_| SwapperError::InvalidRoute)?;

        let request_from = Self::coin_type(&quote.request.from_asset.asset_id());
        let request_to = Self::coin_type(&quote.request.to_asset.asset_id());
        if !coin_type_matches(&request_from, route.input_coin_type()) || !coin_type_matches(&request_to, route.output_coin_type()) {
            return Err(SwapperError::InvalidRoute);
        }

        tx_builder::build_quote_data(&self.sui_client, quote, &route, &Self::referral_fee(), CETUS_CLMM_PUBLISHED_AT).await
    }
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

#[cfg(all(test, feature = "swap_integration_tests"))]
mod swap_integration_tests {
    use super::*;
    use crate::{FetchQuoteData, SwapperQuoteAsset, alien::reqwest_provider::NativeProvider, models::Options};
    use primitives::{AssetId, asset_constants::SUI_USDC_TOKEN_ID};

    const TEST_WALLET: &str = "0x9059c9d089cebc40fbe8c365782ab1285b99959fa386f5a5fc9cdf861a3e0b17";
    const BLUE_TOKEN_ID: &str = "0xe1b45a0e641b9955a20aa0ad1c1f4ad86aad8afb07296d4085e349a50e90bdca::blue::BLUE";

    #[tokio::test]
    async fn test_cetus_clmm_provider_fetch_quote_and_data() -> Result<(), SwapperError> {
        let rpc_provider = Arc::new(NativeProvider::default());
        let provider = CetusClmm::new(rpc_provider);
        let request = QuoteRequest {
            from_asset: SwapperQuoteAsset::from(AssetId::from_chain(Chain::Sui)),
            to_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(SUI_USDC_TOKEN_ID.to_string()))),
            wallet_address: TEST_WALLET.to_string(),
            destination_address: TEST_WALLET.to_string(),
            value: "1500000000".to_string(),
            options: Options::new_with_slippage(50.into()),
        };

        let quote = provider.get_quote(&request).await?;
        let quote_data = provider.get_quote_data(&quote, FetchQuoteData::None).await?;

        assert!(quote.to_value.parse::<u64>().unwrap() > 0);
        assert!(!quote_data.data.is_empty());
        assert!(quote_data.gas_limit.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_cetus_clmm_provider_fetch_quote_usdc_to_sui() -> Result<(), SwapperError> {
        let rpc_provider = Arc::new(NativeProvider::default());
        let provider = CetusClmm::new(rpc_provider);
        let request = QuoteRequest {
            from_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(SUI_USDC_TOKEN_ID.to_string()))),
            to_asset: SwapperQuoteAsset::from(AssetId::from_chain(Chain::Sui)),
            wallet_address: TEST_WALLET.to_string(),
            destination_address: TEST_WALLET.to_string(),
            value: "100000".to_string(),
            options: Options::new_with_slippage(50.into()),
        };

        let quote = provider.get_quote(&request).await?;
        let quote_data = provider.get_quote_data(&quote, FetchQuoteData::None).await?;

        assert!(quote.to_value.parse::<u64>().unwrap() > 0);
        assert!(!quote_data.data.is_empty());
        assert!(quote_data.gas_limit.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_cetus_clmm_provider_discovers_blue_sui_pool() -> Result<(), SwapperError> {
        let rpc_provider = Arc::new(NativeProvider::default());
        let provider = CetusClmm::new(rpc_provider);
        let request = QuoteRequest {
            from_asset: SwapperQuoteAsset::from(AssetId::from_chain(Chain::Sui)),
            to_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(BLUE_TOKEN_ID.to_string()))),
            wallet_address: TEST_WALLET.to_string(),
            destination_address: TEST_WALLET.to_string(),
            value: "100000000".to_string(),
            options: Options::new_with_slippage(100.into()),
        };

        let quote = provider.get_quote(&request).await?;
        assert!(quote.to_value.parse::<u64>().unwrap() > 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_cetus_clmm_provider_routes_usdc_to_blue() -> Result<(), SwapperError> {
        let rpc_provider = Arc::new(NativeProvider::default());
        let provider = CetusClmm::new(rpc_provider);
        let request = QuoteRequest {
            from_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(SUI_USDC_TOKEN_ID.to_string()))),
            to_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(BLUE_TOKEN_ID.to_string()))),
            wallet_address: TEST_WALLET.to_string(),
            destination_address: TEST_WALLET.to_string(),
            value: "100000".to_string(),
            options: Options::new_with_slippage(100.into()),
        };

        let quote = provider.get_quote(&request).await?;
        assert!(quote.to_value.parse::<u64>().unwrap() > 0);
        let route_entry = quote.data.routes.first().unwrap();
        let route: PoolRoute = serde_json::from_str(&route_entry.route_data).unwrap();
        assert!(!route.hops.is_empty() && route.hops.len() <= 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_cetus_clmm_provider_routes_blue_to_usdc() -> Result<(), SwapperError> {
        let rpc_provider = Arc::new(NativeProvider::default());
        let provider = CetusClmm::new(rpc_provider);
        let request = QuoteRequest {
            from_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(BLUE_TOKEN_ID.to_string()))),
            to_asset: SwapperQuoteAsset::from(AssetId::from(Chain::Sui, Some(SUI_USDC_TOKEN_ID.to_string()))),
            wallet_address: TEST_WALLET.to_string(),
            destination_address: TEST_WALLET.to_string(),
            value: "10000000".to_string(),
            options: Options::new_with_slippage(100.into()),
        };

        let quote = provider.get_quote(&request).await?;
        assert!(quote.to_value.parse::<u64>().unwrap() > 0);
        let route_entry = quote.data.routes.first().unwrap();
        let route: PoolRoute = serde_json::from_str(&route_entry.route_data).unwrap();
        assert!(!route.hops.is_empty() && route.hops.len() <= 2);
        Ok(())
    }
}
