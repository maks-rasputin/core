use gem_sui::SUI_COIN_TYPE_FULL;
use primitives::asset_constants::{SUI_CETUS_TOKEN_ID, SUI_USDC_TOKEN_ID};

pub(super) const CETUS_CLMM_PUBLISHED_AT: &str = "0x25ebb9a7c50eb17b3fa9c5a30fb8b5ad8f97caaf4928943acbcff7153dfee5e3";

pub(super) const CETUS_SHARED_INIT_VERSION: u64 = 1_574_190;

pub(super) const CETUS_GLOBAL_CONFIG: &str = "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f";
pub(super) const CETUS_POOLS_REGISTRY: &str = "0xf699e7f2276f5c9a75944b37a0c5b5d9ddfd2471bf6242483b03ab2887d198d0";

pub(super) const MODULE_POOL: &str = "pool";
pub(super) const MODULE_FACTORY: &str = "factory";

pub(super) const FUNCTION_CALCULATE_SWAP_RESULT: &str = "calculate_swap_result";
pub(super) const FUNCTION_FLASH_SWAP: &str = "flash_swap";
pub(super) const FUNCTION_REPAY_FLASH_SWAP: &str = "repay_flash_swap";
pub(super) const FUNCTION_NEW_POOL_KEY: &str = "new_pool_key";
pub(super) const FUNCTION_POOL_SIMPLE_INFO: &str = "pool_simple_info";
pub(super) const FUNCTION_POOL_ID: &str = "pool_id";

pub(super) const MIN_SQRT_PRICE_X64: u128 = 4_295_048_016;
pub(super) const MAX_SQRT_PRICE_X64: u128 = 79_226_673_515_401_279_992_447_579_055;

pub(super) const CETUS_TICK_SPACINGS: &[u32] = &[10, 60, 200];

pub(super) struct KnownPool {
    pub coin_a: &'static str,
    pub coin_b: &'static str,
    pub pool_id: &'static str,
    pub pool_init_version: u64,
}

pub(super) const KNOWN_POOLS: &[KnownPool] = &[
    KnownPool {
        coin_a: SUI_USDC_TOKEN_ID,
        coin_b: SUI_COIN_TYPE_FULL,
        pool_id: "0xb8d7d9e66a60c239e7a60110efcf8de6c705580ed924d0dde141f4a0e2c90105",
        pool_init_version: 373_623_018,
    },
    KnownPool {
        coin_a: SUI_USDC_TOKEN_ID,
        coin_b: SUI_COIN_TYPE_FULL,
        pool_id: "0x413ddc5745aa6398e9da66c4843947e479f4bf63bade39ffc94c9197b433b332",
        pool_init_version: 415_321_657,
    },
    KnownPool {
        coin_a: SUI_USDC_TOKEN_ID,
        coin_b: SUI_COIN_TYPE_FULL,
        pool_id: "0x51e883ba7c0b566a26cbc8a94cd33eb0abd418a77cc1e60ad22fd9b1f29cd2ab",
        pool_init_version: 376_543_995,
    },
    KnownPool {
        coin_a: SUI_USDC_TOKEN_ID,
        coin_b: SUI_COIN_TYPE_FULL,
        pool_id: "0x03d7739b33fe221a830ff101042fa81fd19188feca04a335f7dea4e37c0fca81",
        pool_init_version: 373_502_515,
    },
    KnownPool {
        coin_a: SUI_USDC_TOKEN_ID,
        coin_b: SUI_CETUS_TOKEN_ID,
        pool_id: "0xeee7dcdedebedc97fe8f62ebfe467c26dd36f068e537f071477bf8e0fa84b8d1",
        pool_init_version: 797_958_771,
    },
    KnownPool {
        coin_a: SUI_CETUS_TOKEN_ID,
        coin_b: SUI_COIN_TYPE_FULL,
        pool_id: "0xfb9b72266c119c97d5cb22be5403a68b1323a810a2a97bdb2aa9720148c6128a",
        pool_init_version: 797_928_149,
    },
];
