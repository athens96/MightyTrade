//! Unified broker client facade dispatching seamlessly between Toss and Kiwoom REST APIs.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::errors::ApiResult;
use crate::integrations::kiwoom::KiwoomClient;
use crate::integrations::toss::TossClient;
use crate::users::credentials::BrokerKind;

#[derive(Clone)]
pub enum BrokerClient {
    Toss(Arc<TossClient>),
    Kiwoom(Arc<KiwoomClient>),
}

impl BrokerClient {
    pub fn kind(&self) -> BrokerKind {
        match self {
            Self::Toss(_) => BrokerKind::Toss,
            Self::Kiwoom(_) => BrokerKind::Kiwoom,
        }
    }

    pub fn is_kiwoom(&self) -> bool {
        matches!(self, Self::Kiwoom(_))
    }

    pub async fn request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, String>>,
        json_body: Option<&Value>,
        account_seq: Option<i64>,
    ) -> ApiResult<Option<Value>> {
        match self {
            Self::Toss(c) => c.request(method, path, params, json_body, account_seq).await,
            Self::Kiwoom(c) => c.canonical_request(method, path, params, json_body).await,
        }
    }
}
