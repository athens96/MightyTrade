use chrono::Utc;
use serde_json::{json, Value};

use crate::market::models::Market;
use crate::state::AppState;
use crate::users::credentials;

pub async fn cost_defaults(state: &AppState, user_id: &str, market: Market) -> Value {
    let kr = market == Market::KR;
    let mut commission_pct = 0.015;
    let mut commission_source = "default";

    let is_toss = state
        .config
        .market_providers
        .get(market.as_str())
        .map(|s| s.as_str())
        == Some("toss");
    if is_toss {
        if let Some(creds) = credentials::get_toss(state, user_id) {
            if let Some(seq) = creds.account_seq {
                if let Ok(client) = state.get_toss_client(user_id) {
                    if let Ok(Some(Value::Array(rows))) = client
                        .request("GET", "/api/v1/commissions", None, None, Some(seq))
                        .await
                    {
                        let today = Utc::now().date_naive().to_string();
                        for r in rows {
                            let start_ok = r
                                .get("startDate")
                                .and_then(|v| v.as_str())
                                .map(|s| s.is_empty() || s <= today.as_str())
                                .unwrap_or(true);
                            let end_ok = r
                                .get("endDate")
                                .and_then(|v| v.as_str())
                                .map(|s| s.is_empty() || s >= today.as_str())
                                .unwrap_or(true);
                            if r.get("marketCountry").and_then(|v| v.as_str())
                                == Some(market.as_str())
                                && start_ok
                                && end_ok
                            {
                                if let Some(rate) = r.get("commissionRate").and_then(|v| {
                                    v.as_f64()
                                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                                }) {
                                    commission_pct = rate * 100.0;
                                    commission_source = "toss";
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    json!({
        "capital": if kr { 10_000_000.0 } else { 10_000.0 },
        "currency": if kr { "KRW" } else { "USD" },
        "commission_pct": commission_pct,
        "commission_source": commission_source,
        "tax_pct": if kr { 0.20 } else { 0.0 },
        "slippage_pct": 0.05,
    })
}
