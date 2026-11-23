//! Kiwoom-specific endpoints (Phase 3: HTS Condition Search `CNSRLST` / `CNSRREQ`).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::extractors::AuthUser;
use crate::errors::{ApiError, ApiResult};
use crate::integrations::kiwoom::{parse_kiwoom_abs, parse_kiwoom_signed, strip_kr_prefix};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/conditions", get(list_conditions))
        .route("/conditions/{seq}/matches", get(condition_matches))
}

async fn list_conditions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let client = state.get_kiwoom_client(uid)?;
    let list = client.list_conditions().await?;
    let items: Vec<Value> = list
        .into_iter()
        .map(|(seq, name)| json!({ "seq": seq, "name": name }))
        .collect();
    Ok(Json(json!({
        "is_mock": client.is_mock,
        "count": items.len(),
        "items": items,
    })))
}

async fn condition_matches(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(seq): Path<String>,
) -> ApiResult<Json<Value>> {
    let clean_seq = seq.trim();
    if clean_seq.is_empty()
        || clean_seq.len() > 16
        || !clean_seq.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "invalid_condition_seq",
            "유효한 조건검색식 번호(seq)가 아닙니다",
        ));
    }
    let uid = user["id"].as_str().unwrap_or("");
    let client = state.get_kiwoom_client(uid)?;
    let raw_rows = client.query_condition_matches(clean_seq).await?;
    let items: Vec<Value> = raw_rows
        .iter()
        .filter_map(normalize_condition_row)
        .collect();
    Ok(Json(json!({
        "seq": clean_seq,
        "market": "KR",
        "is_mock": client.is_mock,
        "count": items.len(),
        "items": items,
    })))
}

pub fn normalize_condition_row(row: &Value) -> Option<Value> {
    let raw_code = row
        .get("9001")
        .or_else(|| row.get("stk_cd"))
        .or_else(|| row.get("code"))
        .and_then(|v| v.as_str())?;
    let symbol = strip_kr_prefix(raw_code);
    if symbol.is_empty() {
        return None;
    }
    let name = row
        .get("302")
        .or_else(|| row.get("stk_nm"))
        .or_else(|| row.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(&symbol);
    let price = parse_kiwoom_abs(row.get("10").or_else(|| row.get("cur_prc"))).unwrap_or(0.0);
    let change = parse_kiwoom_signed(row.get("11").or_else(|| row.get("pred_pre"))).unwrap_or(0.0);
    let change_pct = parse_kiwoom_signed(row.get("12").or_else(|| row.get("flu_rt"))).unwrap_or(0.0);
    let volume = parse_kiwoom_abs(row.get("13").or_else(|| row.get("trde_qty"))).unwrap_or(0.0);
    Some(json!({
        "symbol": symbol,
        "name": name,
        "price": price,
        "change": change,
        "change_pct": change_pct,
        "volume": volume,
    }))
}
