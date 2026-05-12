use gem_client::ReqwestClient;
use primitives::swap::{ProxyQuote, ProxyQuoteRequest, SwapQuoteData};
use rocket::serde::json::Json;
use std::sync::Arc;
use swapper::{
    RpcProvider,
    okx::{BASE_URL, OkxClientConfig, OkxProvider},
    proxy::ProxyResponse,
};

pub struct OkxApiClient {
    provider: OkxProvider<ReqwestClient>,
}

impl OkxApiClient {
    pub fn new(config: OkxClientConfig, rpc_provider: Arc<dyn RpcProvider>) -> Self {
        let http = ReqwestClient::new(BASE_URL.to_string(), reqwest::Client::new());
        Self {
            provider: OkxProvider::new(http, config, rpc_provider),
        }
    }
}

#[rocket::post("/swaps/providers/okx/quote", data = "<body>")]
pub async fn post_okx_quote(body: Json<ProxyQuoteRequest>, client: &rocket::State<OkxApiClient>) -> Json<ProxyResponse<ProxyQuote>> {
    Json(client.provider.compute_quote(body.into_inner()).await.into())
}

#[rocket::post("/swaps/providers/okx/quote_data", data = "<body>")]
pub async fn post_okx_quote_data(body: Json<ProxyQuote>, client: &rocket::State<OkxApiClient>) -> Json<ProxyResponse<SwapQuoteData>> {
    Json(client.provider.compute_quote_data(body.into_inner()).await.into())
}
