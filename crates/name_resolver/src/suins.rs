use crate::client::NameClient;
use crate::model::NameQuery;
use async_trait::async_trait;
use primitives::NameProvider;
use primitives::chain::Chain;
use std::error::Error;
use sui_rpc::proto::sui::rpc::v2::{LookupNameRequest, name_service_client::NameServiceClient};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

pub struct SuinsClient {
    client: Result<NameServiceClient<Channel>, String>,
}

impl SuinsClient {
    pub fn new(api_url: String) -> Self {
        Self {
            client: Self::new_client(api_url).map_err(|error| error.to_string()),
        }
    }

    fn new_client(api_url: String) -> Result<NameServiceClient<Channel>, Box<dyn Error + Send + Sync>> {
        let endpoint = Endpoint::from_shared(api_url)?.tls_config(ClientTlsConfig::new().with_enabled_roots())?;
        Ok(NameServiceClient::new(endpoint.connect_lazy()))
    }
}

#[async_trait]
impl NameClient for SuinsClient {
    fn provider(&self) -> NameProvider {
        NameProvider::Suins
    }

    async fn resolve(&self, query: &NameQuery, _chain: Chain) -> Result<String, Box<dyn Error + Send + Sync>> {
        let mut client = self.client.clone()?;
        let response = client.lookup_name(LookupNameRequest::new(query.domain.clone())).await?.into_inner();
        Ok(response.record.and_then(|record| record.target_address).ok_or("SuiNS record has no target address")?)
    }

    fn domains(&self) -> Vec<&'static str> {
        vec!["sui"]
    }

    fn chains(&self) -> Vec<Chain> {
        vec![Chain::Sui]
    }
}
