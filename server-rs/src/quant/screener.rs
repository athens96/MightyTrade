//! Quant Stock Screener (7 presets, KR/US universes, Rayon-parallel strategy-fit, fundamentals, KR flows).

use chrono::Utc;
use rayon::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::errors::{ApiError, ApiResult};
use crate::market::{self, extras, models::{Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::quant::engine::{run_backtest, Bars, Costs, RiskRules};
use crate::quant::fundamentals;
use crate::quant::indicators as ind;
use crate::quant::screener_data as sdata;
use crate::quant::strategies::{all_specs, get_spec, StrategySpec};
use crate::state::AppState;

const MAX_CUSTOM_SYMBOLS: usize = 50;

const KR_FALLBACK_KOSPI: &[(&str, &str)] = &[
    ("005930", "삼성전자"), ("000660", "SK하이닉스"), ("373220", "LG에너지솔루션"),
    ("207940", "삼성바이오로직스"), ("005380", "현대차"), ("000270", "기아"),
    ("068270", "셀트리온"), ("105560", "KB금융"), ("055550", "신한지주"),
    ("086790", "하나금융지주"), ("005490", "POSCO홀딩스"), ("035420", "NAVER"),
    ("035720", "카카오"), ("006400", "삼성SDI"), ("051910", "LG화학"),
    ("012330", "현대모비스"), ("028260", "삼성물산"), ("033780", "KT&G"),
    ("017670", "SK텔레콤"), ("030200", "KT"), ("015760", "한국전력"),
    ("012450", "한화에어로스페이스"), ("329180", "HD현대중공업"), ("034020", "두산에너빌리티"),
    ("010130", "고려아연"), ("009150", "삼성전기"), ("018260", "삼성에스디에스"),
    ("032830", "삼성생명"), ("000810", "삼성화재"), ("138040", "메리츠금융지주"),
];

const KR_FALLBACK_KOSDAQ: &[(&str, &str)] = &[
    ("196170", "알테오젠"), ("247540", "에코프로비엠"), ("086520", "에코프로"),
    ("028300", "HLB"), ("277810", "레인보우로보틱스"), ("058470", "리노공업"),
    ("214150", "클래시스"), ("039030", "이오테크닉스"), ("035900", "JYP Ent."),
    ("041510", "에스엠"), ("257720", "실리콘투"), ("293490", "카카오게임즈"),
    ("112040", "위메이드"), ("067160", "SOOP"), ("145020", "휴젤"),
];

const KR_ETF_LIST: &[(&str, &str)] = &[
    ("069500", "KODEX 200"), ("229200", "코스닥150"), ("360750", "미국 S&P500"),
    ("133690", "미국 나스닥100"), ("381170", "미국테크TOP10"), ("381180", "미국필라델피아반도체"),
    ("091160", "반도체"), ("091180", "자동차"), ("091170", "은행"),
    ("102970", "증권"), ("305720", "2차전지산업"), ("244580", "바이오"),
    ("449450", "방산"), ("466920", "조선"), ("266360", "미디어&엔터"),
    ("139260", "IT"), ("140700", "보험"), ("117700", "건설"),
    ("458730", "미국배당다우존스"), ("161510", "플러스 고배당주"), ("211560", "배당성장"),
    ("329200", "리츠부동산인프라"), ("148070", "국고채10년"), ("114260", "국고채3년"),
    ("153130", "단기채권"), ("305080", "미국채10년선물"), ("453850", "미국30년국채"),
    ("132030", "골드선물"), ("261220", "WTI원유"), ("261240", "미국달러선물"),
];

const US_SP100_LIST: &[(&str, &str)] = &[
    ("AAPL", "Apple"), ("MSFT", "Microsoft"), ("NVDA", "NVIDIA"), ("AVGO", "Broadcom"),
    ("ORCL", "Oracle"), ("CRM", "Salesforce"), ("ADBE", "Adobe"), ("CSCO", "Cisco"),
    ("ACN", "Accenture"), ("IBM", "IBM"), ("QCOM", "Qualcomm"), ("TXN", "Texas Instruments"),
    ("AMD", "AMD"), ("AMAT", "Applied Materials"), ("INTU", "Intuit"), ("NOW", "ServiceNow"),
    ("MU", "Micron"), ("LRCX", "Lam Research"), ("ADI", "Analog Devices"), ("KLAC", "KLA"),
    ("GOOGL", "Alphabet"), ("META", "Meta"), ("NFLX", "Netflix"), ("DIS", "Disney"),
    ("TMUS", "T-Mobile"), ("VZ", "Verizon"), ("T", "AT&T"), ("CMCSA", "Comcast"),
    ("AMZN", "Amazon"), ("TSLA", "Tesla"), ("HD", "Home Depot"), ("MCD", "McDonald's"),
    ("LOW", "Lowe's"), ("BKNG", "Booking"), ("TJX", "TJX"), ("NKE", "Nike"),
    ("SBUX", "Starbucks"), ("GM", "General Motors"), ("F", "Ford"),
    ("WMT", "Walmart"), ("COST", "Costco"), ("PG", "Procter & Gamble"), ("KO", "Coca-Cola"),
    ("PEP", "PepsiCo"), ("PM", "Philip Morris"), ("MO", "Altria"), ("CL", "Colgate-Palmolive"),
    ("MDLZ", "Mondelez"), ("KMB", "Kimberly-Clark"), ("TGT", "Target"),
    ("LLY", "Eli Lilly"), ("UNH", "UnitedHealth"), ("JNJ", "Johnson & Johnson"), ("ABBV", "AbbVie"),
    ("MRK", "Merck"), ("TMO", "Thermo Fisher"), ("ABT", "Abbott"), ("ISRG", "Intuit Surgical"),
    ("AMGN", "Amgen"), ("PFE", "Pfizer"), ("DHR", "Danaher"), ("BMY", "Bristol-Myers"),
    ("GILD", "Gilead"), ("CVS", "CVS Health"), ("MDT", "Medtronic"),
    ("JPM", "JPMorgan"), ("V", "Visa"), ("MA", "Mastercard"), ("BAC", "Bank of America"),
    ("WFC", "Wells Fargo"), ("GS", "Goldman Sachs"), ("MS", "Morgan Stanley"), ("BLK", "BlackRock"),
    ("SCHW", "Charles Schwab"), ("AXP", "American Express"), ("C", "Citigroup"), ("USB", "U.S. Bancorp"),
    ("CB", "Chubb"), ("PGR", "Progressive"), ("MET", "MetLife"),
    ("GE", "GE Aerospace"), ("CAT", "Caterpillar"), ("RTX", "RTX"), ("HON", "Honeywell"),
    ("UNP", "Union Pacific"), ("LMT", "Lockheed Martin"), ("DE", "Deere"), ("UPS", "UPS"),
    ("BA", "Boeing"), ("ETN", "Eaton"),
    ("XOM", "Exxon Mobil"), ("CVX", "Chevron"), ("COP", "ConocoPhillips"), ("SLB", "Schlumberger"),
    ("EOG", "EOG Resources"), ("PSX", "Phillips 66"),
    ("NEE", "NextEra Energy"), ("SO", "Southern Co"), ("DUK", "Duke Energy"), ("LIN", "Linde"),
    ("FCX", "Freeport-McMoRan"), ("AMT", "American Tower"), ("PLD", "Prologis"), ("O", "Realty Income"),
];

const US_NASDAQ50_LIST: &[(&str, &str)] = &[
    ("AAPL", "Apple"), ("MSFT", "Microsoft"), ("NVDA", "NVIDIA"), ("AMZN", "Amazon"),
    ("GOOGL", "Alphabet"), ("META", "Meta"), ("TSLA", "Tesla"), ("AVGO", "Broadcom"),
    ("COST", "Costco"), ("NFLX", "Netflix"), ("AMD", "AMD"), ("TMUS", "T-Mobile"),
    ("CSCO", "Cisco"), ("PEP", "PepsiCo"), ("LIN", "Linde"), ("ADBE", "Adobe"),
    ("QCOM", "Qualcomm"), ("TXN", "Texas Instruments"), ("ISRG", "Intuitive Surgical"), ("INTU", "Intuit"),
    ("AMGN", "Amgen"), ("AMAT", "Applied Materials"), ("BKNG", "Booking"), ("HON", "Honeywell"),
    ("PANW", "Palo Alto"), ("CRWD", "CrowdStrike"), ("SNPS", "Synopsys"), ("CDNS", "Cadence"),
    ("KLAC", "KLA"), ("LRCX", "Lam Research"), ("MU", "Micron"), ("MRVL", "Marvell"),
    ("ADI", "Analog Devices"), ("ASML", "ASML"), ("PLTR", "Palantir"), ("APP", "AppLovin"),
    ("MELI", "MercadoLibre"), ("SHOP", "Shopify"), ("DDOG", "Datadog"), ("ZS", "Zscaler"),
    ("FTNT", "Fortinet"), ("GILD", "Gilead"), ("REGN", "Regeneron"), ("VRTX", "Vertex"),
    ("MDLZ", "Mondelez"), ("SBUX", "Starbucks"), ("PYPL", "PayPal"), ("ABNB", "Airbnb"),
    ("ADP", "ADP"), ("CTAS", "Cintas"),
];

const US_DIVIDEND40_LIST: &[(&str, &str)] = &[
    ("JNJ", "Johnson & Johnson"), ("KO", "Coca-Cola"), ("PEP", "PepsiCo"), ("PG", "Procter & Gamble"),
    ("XOM", "Exxon Mobil"), ("CVX", "Chevron"), ("ABBV", "AbbVie"), ("MRK", "Merck"),
    ("PFE", "Pfizer"), ("BMY", "Bristol-Myers"), ("GILD", "Gilead"), ("VZ", "Verizon"),
    ("T", "AT&T"), ("MO", "Altria"), ("PM", "Philip Morris"), ("O", "Realty Income"),
    ("AMT", "American Tower"), ("SPG", "Simon Property"), ("PSA", "Public Storage"), ("VICI", "VICI Properties"),
    ("SO", "Southern Co"), ("DUK", "Duke Energy"), ("NEE", "NextEra Energy"), ("ED", "Consolidated Edison"),
    ("D", "Dominion Energy"), ("KMB", "Kimberly-Clark"), ("CL", "Colgate-Palmolive"), ("GIS", "General Mills"),
    ("MCD", "McDonald's"), ("HD", "Home Depot"), ("LOW", "Lowe's"), ("IBM", "IBM"),
    ("LMT", "Lockheed Martin"), ("RTX", "RTX"), ("CAT", "Caterpillar"), ("UPS", "UPS"),
    ("JPM", "JPMorgan"), ("USB", "U.S. Bancorp"), ("PNC", "PNC Financial"), ("MET", "MetLife"),
];

const US_ETF_LIST: &[(&str, &str)] = &[
    ("SPY", "미국 S&P500"), ("QQQ", "나스닥100"), ("IWM", "미국 소형주"), ("DIA", "다우존스"),
    ("VTI", "미국 전체주식"), ("VEA", "선진국 주식"), ("VWO", "신흥국 주식"), ("EWY", "한국 MSCI"),
    ("XLK", "기술 (IT)"), ("XLV", "헬스케어"), ("XLF", "금융"), ("XLE", "에너지"),
    ("XLI", "산업재"), ("XLY", "경기소비재"), ("XLP", "필수소비재"), ("XLU", "유틸리티"),
    ("XLRE", "부동산 리츠"), ("XLB", "소재"), ("XLC", "커뮤니케이션"), ("SMH", "반도체"),
    ("SOXX", "필라델피아 반도체"), ("ITA", "미국 방산"), ("XBI", "바이오테크"), ("SCHD", "미국 배당성장"),
    ("VYM", "고배당 주식"), ("JEPI", "커버드콜 인컴"), ("TLT", "미국 장기국채"), ("IEF", "미국 중기국채"),
    ("SHY", "미국 단기국채"), ("GLD", "금"), ("DBC", "원자재"),
];

pub struct PresetMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub theory: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub needs_fundamentals: bool,
    pub kr_only: bool,
    pub uses_strategy: bool,
    pub recommended_strategy: &'static str,
    pub recommended_portfolio: &'static str,
}

pub const PRESETS: &[PresetMeta] = &[
    PresetMeta {
        id: "momentum_quality",
        name: "부드러운 상승 추세 TOP",
        theory: "추세 모멘텀 + 저변동성 · 롤러코스터 말고 '꾸준히 우상향하는' 주도주 찾기",
        summary: "최근 3·6·12개월 동안 수익률이 좋으면서도 중간에 덜 출렁인 우량 추세주를 뽑습니다",
        description: "최근 3·6·12개월 수익률을 60일 변동성으로 나눈 위험조정 모멘텀과 52주 신고가 근접도, 200일 평균선 위 조건으로 우량 추세주를 선정합니다.",
        needs_fundamentals: false, kr_only: false, uses_strategy: false,
        recommended_strategy: "ts_momentum", recommended_portfolio: "sector_rotation",
    },
    PresetMeta {
        id: "oversold_bounce",
        name: "잠깐 너무 빠진 우량주 (반등 후보)",
        theory: "래리 코너스 RSI(2) + IBS · 장기 상승세 우량주의 일시적 세일 구간 포착",
        summary: "큰 흐름은 상승(200일선 위)인데 최근 2~5일 사이 과하게 떨어져 반등 가능성이 높은 종목을 찾습니다",
        description: "200일 평균선 위 장기 상승 추세 종목 중 RSI(2) ≤ 25 또는 IBS ≤ 0.25 과매도 반등 후보를 선별합니다.",
        needs_fundamentals: false, kr_only: false, uses_strategy: false,
        recommended_strategy: "rsi2", recommended_portfolio: "equal_weight",
    },
    PresetMeta {
        id: "smart_money",
        name: "외국인·기관 쌍끌이 수급 (국내 전용)",
        theory: "스마트 머니 추종 · 큰손(외국인 + 기관)이 동시에 꾸준히 사 모으는 종목",
        summary: "최근 5일·20일 동안 외국인과 기관이 함께 순매수하고 외국인 보유 비중이 늘어나는 국내 종목을 추출합니다",
        description: "최근 20거래일 외국인·기관 동시 순매수(쌍끌이)와 지분 증가가 뚜렷한 국내 주도주를 추출합니다.",
        needs_fundamentals: false, kr_only: true, uses_strategy: false,
        recommended_strategy: "ma_cross", recommended_portfolio: "hrp",
    },
    PresetMeta {
        id: "strategy_fit",
        name: "전략 궁합 TOP (내 봇과 찰떡 종목)",
        theory: "전략-종목 궁합 검증 · 선택한 봇 전략이 최근 2년간 가장 잘 먹힌 종목 찾기",
        summary: "내가 고른 스윙·단타 전략을 후보 종목 전체에 한꺼번에 백테스트해 성적이 제일 좋았던 종목을 뽑습니다",
        description: "후보 종목 전체에 선택한 일봉 전략을 최근 2년치로 일괄 백테스트해 샤프·수익률·승률이 가장 뛰어난 종목을 선별합니다.",
        needs_fundamentals: false, kr_only: false, uses_strategy: true,
        recommended_strategy: "rsi2", recommended_portfolio: "equal_weight",
    },
    PresetMeta {
        id: "magic_formula",
        name: "마법공식 (돈 잘 버는 저평가 우량주)",
        theory: "조엘 그린블라트 · '수익성 좋은 우량 회사(고ROE)'를 '싼 가격(저PER)'에 사기",
        summary: "이익을 잘 내는 능력(ROE) 순위와 주가가 싼 정도(PER) 순위를 합쳐 최고의 가성비 우량주를 뽑습니다",
        description: "조엘 그린블라트의 마법공식 기반으로 ROE 상위 순위와 PER 하위 순위를 합산해 저평가 우량주를 선정합니다.",
        needs_fundamentals: true, kr_only: false, uses_strategy: false,
        recommended_strategy: "ma_cross", recommended_portfolio: "equal_weight",
    },
    PresetMeta {
        id: "dividend_lowvol",
        name: "고배당 + 덜 출렁이는 안정주",
        theory: "배당 수익 + 저변동성 · 이자처럼 배당 받으면서 마음 편히 들고 갈 종목 찾기",
        summary: "배당수익률이 높으면서도 평소 주가 흔들림이 적은 안정적인 배당주를 뽑습니다",
        description: "연 배당률 2.5% 이상이면서 연 변동성이 낮은 안정적 고배당주를 발굴합니다.",
        needs_fundamentals: true, kr_only: false, uses_strategy: false,
        recommended_strategy: "bollinger", recommended_portfolio: "risk_parity",
    },
    PresetMeta {
        id: "multifactor",
        name: "멀티팩터 종합 (가치·우량·상승 3박자)",
        theory: "파마-프렌치 · 싸고(가치) + 튼튼하고(우량) + 잘 오르는(모멘텀) 만능 우량주",
        summary: "저평가(PER·PBR) + 돈 잘 버는 힘(ROE) + 최근 상승 추세(모멘텀) 3가지를 골고루 갖춘 종목을 뽑습니다",
        description: "가치(PER·PBR) + 수익성(ROE) + 모멘텀(6·12개월 추세) 3요소를 균형 채점해 만능 우량주를 선정합니다.",
        needs_fundamentals: true, kr_only: false, uses_strategy: false,
        recommended_strategy: "ts_momentum", recommended_portfolio: "equal_weight",
    },
];

pub fn get_preset(id: &str) -> Option<&'static PresetMeta> {
    PRESETS.iter().find(|p| p.id == id)
}

pub fn is_valid_universe(market: &str, universe: &str) -> bool {
    if matches!(universe, "kr_bluechip" | "us_bluechip") {
        return true;
    }
    match market {
        "KR" => matches!(
            universe,
            "kr_all_150" | "kr_kospi_120" | "kr_kosdaq_100" | "kr_etf" | "watchlist" | "custom"
        ),
        "US" => matches!(
            universe,
            "us_sp100" | "us_nasdaq_50" | "us_dividend_40" | "us_etf" | "watchlist" | "custom"
        ),
        _ => false,
    }
}

pub fn presets_meta() -> Value {
    let presets: Vec<Value> = PRESETS
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "theory": p.theory,
                "summary": p.summary,
                "description": p.description,
                "needs_fundamentals": p.needs_fundamentals,
                "kr_only": p.kr_only,
                "uses_strategy": p.uses_strategy,
                "recommended_strategy": p.recommended_strategy,
                "recommended_portfolio": p.recommended_portfolio,
                "guide": {
                    "background": p.description,
                    "setup": ["후보 묶음을 고른 뒤 [종목 발굴 실행]을 누르세요."],
                    "execution": ["고속 시세·재무·수급 스캔 후 조건 충족 종목을 상위에 배치합니다."],
                    "caution": "단일 종목 몰빵보다 상위 3~5종목 분산 및 백테스트 병행을 권장합니다."
                }
            })
        })
        .collect();
    let daily_strategies: Vec<Value> = all_specs()
        .into_iter()
        .filter(|s| s.timeframe == "1d")
        .map(|s| json!({ "id": s.id, "name": s.name, "category": s.category }))
        .collect();

    json!({
        "presets": presets,
        "universes": {
            "KR": [
                {"id": "kr_all_150", "name": "국내 코스피·코스닥 핵심 150종", "description": "실시간 시가총액 상위 코스피 100 + 코스닥 50 보통주 (우선주·스팩 자동 제외)", "count": 150, "is_etf": false},
                {"id": "kr_kospi_120", "name": "코스피 시총 상위 120종", "description": "실시간 코스피 시가총액 상위 대형·중형 보통주 120종목", "count": 120, "is_etf": false},
                {"id": "kr_kosdaq_100", "name": "코스닥 시총 상위 100종", "description": "실시간 코스닥 시가총액 상위 성장·중소형 보통주 100종목", "count": 100, "is_etf": false},
                {"id": "kr_etf", "name": "국내 대표 섹터·자산 ETF (30종)", "description": "지수·미국·반도체·방산·조선·배당·채권·금·원유 대표 ETF 30종목", "count": KR_ETF_LIST.len(), "is_etf": true},
                {"id": "watchlist", "name": "내 관심종목", "description": "시세 화면에서 저장해 둔 내 국내 관심종목 목록", "count": 0, "is_etf": false},
                {"id": "custom", "name": "직접 입력 종목", "description": "원하는 종목 코드를 쉼표로 입력해 비교 (최대 50종목)", "count": 0, "is_etf": false}
            ],
            "US": [
                {"id": "us_sp100", "name": format!("미국 S&P500 핵심 대형주 ({}종)", US_SP100_LIST.len()), "description": "11개 전 업종 핵심 대형주", "count": US_SP100_LIST.len(), "is_etf": false},
                {"id": "us_nasdaq_50", "name": "미국 나스닥·AI·성장주 (50종)", "description": "반도체·소프트웨어·보안·바이오 등 미국 핵심 성장주 50종목", "count": US_NASDAQ50_LIST.len(), "is_etf": false},
                {"id": "us_dividend_40", "name": "미국 고배당·배당귀족·가치주 (40종)", "description": "꾸준한 배당을 주는 미국 배당귀족·에너지·유틸리티·리츠·금융 40종목", "count": US_DIVIDEND40_LIST.len(), "is_etf": false},
                {"id": "us_etf", "name": "미국 대표 섹터·자산 ETF (31종)", "description": "미국 지수·11개 전 섹터·반도체·방산·배당·국채·금·원자재 대표 ETF 31종목", "count": US_ETF_LIST.len(), "is_etf": true},
                {"id": "watchlist", "name": "내 관심종목", "description": "시세 화면에서 저장해 둔 내 미국 관심종목 목록", "count": 0, "is_etf": false},
                {"id": "custom", "name": "직접 입력 종목", "description": "원하는 미국 티커를 쉼표로 입력해 비교 (최대 50종목)", "count": 0, "is_etf": false}
            ]
        },
        "daily_strategies": daily_strategies,
    })
}

fn mock_flow(symbol: &str) -> Value {
    let digest = Sha256::digest(format!("KR:{symbol}:flow").as_bytes());
    let mut h = 0u64;
    for &b in &digest[..6] {
        h = (h << 8) | (b as u64);
    }
    let f5 = ((h % 900) as i64 - 380) * 1000;
    let i5 = (((h >> 12) % 900) as i64 - 380) * 1000;
    let f20 = f5 * 3 + (((h >> 24) % 500) as i64 - 220) * 1000;
    let i20 = i5 * 3 + (((h >> 32) % 500) as i64 - 220) * 1000;
    let f_delta = ((((h >> 40) % 160) as f64 - 70.0) / 100.0 * 100.0).round() / 100.0;
    json!({
        "flow_5d_for": f5,
        "flow_5d_inst": i5,
        "flow_20d_for": f20,
        "flow_20d_inst": i20,
        "foreign_delta_pct": f_delta,
    })
}

pub async fn resolve_candidates(
    state: &AppState,
    market: Market,
    universe_id: &str,
    custom_symbols: Option<&[String]>,
    is_mock: bool,
    user_id: &str,
) -> ApiResult<(String, Vec<(String, String)>)> {
    let m = market.as_str();
    let uni = match universe_id {
        "kr_bluechip" => "kr_all_150",
        "us_bluechip" => "us_sp100",
        other => other,
    };

    if uni == "custom" {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for s in custom_symbols.unwrap_or(&[]) {
            let sym = s.trim().to_uppercase();
            if sym.is_empty() || seen.contains(&sym) {
                continue;
            }
            if !market.validate_symbol(&sym) {
                return Err(ApiError::bad_request(
                    "invalid_symbol",
                    format!("{sym}은(는) 올바른 {m} 종목 코드가 아닙니다"),
                ));
            }
            seen.insert(sym.clone());
            out.push((sym.clone(), sym));
            if out.len() >= MAX_CUSTOM_SYMBOLS {
                break;
            }
        }
        if out.len() < 2 {
            return Err(ApiError::bad_request(
                "too_few_symbols",
                "직접 입력 종목은 최소 2개 이상 입력하세요",
            ));
        }
        return Ok((uni.to_string(), out));
    }

    if uni == "watchlist" {
        let rec = state.store.get("watchlists", user_id);
        let default_wl = json!([
            {"market": "KR", "symbol": "005930", "name": "삼성전자"},
            {"market": "KR", "symbol": "000660", "name": "SK하이닉스"},
            {"market": "US", "symbol": "AAPL", "name": "Apple"},
            {"market": "US", "symbol": "NVDA", "name": "NVIDIA"}
        ]);
        let items = rec
            .as_ref()
            .and_then(|r| r.get("items"))
            .unwrap_or(&default_wl)
            .as_array()
            .cloned()
            .unwrap_or_default();
        let out: Vec<(String, String)> = items
            .into_iter()
            .filter(|it| it.get("market").and_then(|v| v.as_str()) == Some(m))
            .filter_map(|it| {
                let s = it.get("symbol")?.as_str()?.to_string();
                let n = it.get("name").and_then(|v| v.as_str()).filter(|x| !x.is_empty()).unwrap_or(&s).to_string();
                Some((s, n))
            })
            .take(MAX_CUSTOM_SYMBOLS)
            .collect();
        if out.is_empty() {
            return Err(ApiError::bad_request(
                "empty_watchlist",
                format!("관심종목에 저장된 {m} 종목이 없습니다. 시세 화면에서 종목을 추가하거나 대표 묶음을 선택하세요"),
            ));
        }
        return Ok((uni.to_string(), out));
    }

    let to_vec = |slice: &[(&str, &str)]| -> Vec<(String, String)> {
        slice.iter().map(|(a, b)| ((*a).to_string(), (*b).to_string())).collect()
    };

    match (market, uni) {
        (Market::KR, "kr_etf") => Ok((uni.to_string(), to_vec(KR_ETF_LIST))),
        (Market::KR, "kr_kospi_120") => {
            let live = if is_mock { vec![] } else { sdata::fetch_kr_market_leaders("KOSPI", 120).await };
            Ok((uni.to_string(), if live.is_empty() { to_vec(KR_FALLBACK_KOSPI) } else { live }))
        }
        (Market::KR, "kr_kosdaq_100") => {
            let live = if is_mock { vec![] } else { sdata::fetch_kr_market_leaders("KOSDAQ", 100).await };
            Ok((uni.to_string(), if live.is_empty() { to_vec(KR_FALLBACK_KOSDAQ) } else { live }))
        }
        (Market::KR, "kr_all_150") => {
            if is_mock {
                let mut v = to_vec(KR_FALLBACK_KOSPI);
                v.extend(to_vec(KR_FALLBACK_KOSDAQ));
                return Ok((uni.to_string(), v));
            }
            let kospi_live = sdata::fetch_kr_market_leaders("KOSPI", 100).await;
            let kosdaq_live = sdata::fetch_kr_market_leaders("KOSDAQ", 50).await;
            let mut merged = if kospi_live.is_empty() { to_vec(KR_FALLBACK_KOSPI) } else { kospi_live };
            let seen: HashSet<String> = merged.iter().map(|x| x.0.clone()).collect();
            let kd = if kosdaq_live.is_empty() { to_vec(KR_FALLBACK_KOSDAQ) } else { kosdaq_live };
            for item in kd {
                if !seen.contains(&item.0) {
                    merged.push(item);
                }
            }
            Ok((uni.to_string(), merged))
        }
        (Market::US, "us_sp100") => Ok((uni.to_string(), to_vec(US_SP100_LIST))),
        (Market::US, "us_nasdaq_50") => Ok((uni.to_string(), to_vec(US_NASDAQ50_LIST))),
        (Market::US, "us_dividend_40") => Ok((uni.to_string(), to_vec(US_DIVIDEND40_LIST))),
        (Market::US, "us_etf") => Ok((uni.to_string(), to_vec(US_ETF_LIST))),
        _ => Err(ApiError::bad_request(
            "unknown_universe",
            "알 수 없는 후보 종목 묶음입니다",
        )),
    }
}

fn pct_ranks(values: &[Option<f64>], higher_is_better: bool) -> Vec<Option<f64>> {
    let mut valid: Vec<(f64, usize)> = values
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.filter(|x| x.is_finite()).map(|x| (x, i)))
        .collect();
    let mut out = vec![None; values.len()];
    if valid.is_empty() {
        return out;
    }
    if valid.len() == 1 {
        out[valid[0].1] = Some(70.0);
        return out;
    }
    valid.sort_by(|a, b| {
        if higher_is_better {
            a.0.total_cmp(&b.0)
        } else {
            b.0.total_cmp(&a.0)
        }
    });
    let denom = (valid.len() - 1) as f64;
    for (rank_pos, &(_, orig_idx)) in valid.iter().enumerate() {
        out[orig_idx] = Some(rank_pos as f64 / denom * 100.0);
    }
    out
}

fn blend(scores: &[(Option<f64>, f64)]) -> Option<f64> {
    let mut num = 0.0;
    let mut den = 0.0;
    for &(val, w) in scores {
        if let Some(v) = val.filter(|x| x.is_finite()) {
            num += v * w;
            den += w;
        }
    }
    if den > 0.0 { Some(num / den) } else { None }
}

fn round_n(v: Option<f64>, digits: i32) -> Option<f64> {
    let x = v.filter(|f| f.is_finite())?;
    let f = 10f64.powi(digits);
    Some((x * f).round() / f)
}

pub fn compute_row_stats(bars: &Bars, fit_spec: Option<&StrategySpec>, capital: f64, costs: Costs) -> Value {
    let n = bars.len();
    let c = &bars.close;
    let h = &bars.high;
    let lo = &bars.low;
    let v = &bars.volume;
    let last = c[n - 1];

    let ret = |days: usize| -> Option<f64> {
        if n <= days || c[n - 1 - days] <= 0.0 {
            None
        } else {
            Some((last / c[n - 1 - days] - 1.0) * 100.0)
        }
    };
    let vw = (n - 1).min(60);
    let rets: Vec<f64> = (n - vw..n)
        .filter(|&k| c[k - 1] > 0.0)
        .map(|k| c[k] / c[k - 1] - 1.0)
        .collect();
    let vol60 = if rets.len() >= 5 {
        let mu = rets.iter().sum::<f64>() / rets.len() as f64;
        let sd = (rets.iter().map(|r| (r - mu).powi(2)).sum::<f64>() / (rets.len() - 1) as f64).sqrt();
        Some(sd * 252.0f64.sqrt() * 100.0)
    } else {
        None
    };
    let rsi2 = ind::rsi(c, 2).last().copied().flatten();
    let rng = h[n - 1] - lo[n - 1];
    let ibs = if rng > 0.0 { (last - lo[n - 1]) / rng } else { 0.5 };
    let ma_win = n.min(200);
    let ma200 = if ma_win >= 20 {
        c[n - ma_win..].iter().sum::<f64>() / ma_win as f64
    } else {
        last
    };
    let above_ma200 = last >= ma200;
    let hi_52w = h[n - n.min(252)..]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let near_52w = if hi_52w > 0.0 { Some(last / hi_52w * 100.0) } else { None };
    let vol_ratio = if n >= 20 {
        let v5 = v[n - 5..].iter().sum::<f64>() / 5.0;
        let v20 = v[n - 20..].iter().sum::<f64>() / 20.0;
        if v20 > 0.0 { Some(v5 / v20) } else { None }
    } else {
        None
    };

    let mut out = json!({
        "price": (last * 100.0).round() / 100.0,
        "change_1d_pct": round_n(ret(1), 2),
        "ret_1m_pct": round_n(ret(21), 2),
        "ret_3m_pct": round_n(ret(63), 2),
        "ret_6m_pct": round_n(ret(126), 2),
        "ret_12m_pct": round_n(ret(252), 2),
        "vol_60d_pct": round_n(vol60, 2),
        "rsi2": round_n(rsi2, 1),
        "ibs": round_n(Some(ibs), 2),
        "above_ma200": above_ma200,
        "near_52w_pct": round_n(near_52w, 1),
        "vol_ratio_20d": round_n(vol_ratio, 2),
    });

    if let Some(spec) = fit_spec {
        let obj = out.as_object_mut().unwrap();
        let mut set_empty = || {
            obj.insert("fit_return_pct".to_string(), Value::Null);
            obj.insert("fit_cagr_pct".to_string(), Value::Null);
            obj.insert("fit_mdd_pct".to_string(), Value::Null);
            obj.insert("fit_sharpe".to_string(), Value::Null);
            obj.insert("fit_win_rate".to_string(), Value::Null);
            obj.insert("fit_trades".to_string(), json!(0));
        };
        if let Ok(mut strat) = (spec.build)(&json!({})) {
            let start = strat.warmup().max(bars.len().saturating_sub(504));
            if start < bars.len().saturating_sub(20) {
                if let Ok(res) = run_backtest(
                    bars,
                    strat.as_mut(),
                    capital,
                    costs,
                    start,
                    true,
                    RiskRules::default(),
                    None,
                    "off",
                ) {
                    let m = &res["metrics"];
                    obj.insert("fit_return_pct".to_string(), m.get("total_return_pct").cloned().unwrap_or(Value::Null));
                    obj.insert("fit_cagr_pct".to_string(), m.get("cagr_pct").cloned().unwrap_or(Value::Null));
                    obj.insert("fit_mdd_pct".to_string(), m.get("mdd_pct").cloned().unwrap_or(Value::Null));
                    obj.insert("fit_sharpe".to_string(), m.get("sharpe").cloned().unwrap_or(Value::Null));
                    obj.insert("fit_win_rate".to_string(), m.get("win_rate_pct").cloned().unwrap_or(Value::Null));
                    obj.insert("fit_trades".to_string(), json!(m.get("trades").and_then(|v| v.as_u64()).unwrap_or(0)));
                } else {
                    set_empty();
                }
            } else {
                set_empty();
            }
        } else {
            set_empty();
        }
    }
    out
}

pub fn score_and_explain(preset_id: &str, rows: &mut Vec<Value>) {
    if rows.is_empty() {
        return;
    }
    let f = |r: &Value, k: &str| r.get(k).and_then(|v| v.as_f64());
    let b = |r: &Value, k: &str| r.get(k).and_then(|v| v.as_bool()).unwrap_or(false);

    match preset_id {
        "momentum_quality" => {
            let raw_mom: Vec<Option<f64>> = rows
                .iter()
                .map(|r| {
                    let parts: Vec<f64> = [f(r, "ret_3m_pct"), f(r, "ret_6m_pct"), f(r, "ret_12m_pct")]
                        .into_iter()
                        .flatten()
                        .collect();
                    if parts.is_empty() {
                        return None;
                    }
                    let avg = parts.iter().sum::<f64>() / parts.len() as f64;
                    let vol = f(r, "vol_60d_pct").unwrap_or(20.0).max(8.0);
                    Some(avg / vol)
                })
                .collect();
            let p_mom = pct_ranks(&raw_mom, true);
            let p_52 = pct_ranks(&rows.iter().map(|r| f(r, "near_52w_pct")).collect::<Vec<_>>(), true);
            let p_vol = pct_ranks(&rows.iter().map(|r| f(r, "vol_60d_pct")).collect::<Vec<_>>(), false);
            for (i, r) in rows.iter_mut().enumerate() {
                let base = blend(&[(p_mom[i], 0.60), (p_52[i], 0.25), (p_vol[i], 0.15)]).unwrap_or(0.0);
                let above = b(r, "above_ma200");
                let sc = ((base + if above { 8.0 } else { -12.0 }).clamp(0.0, 100.0) * 10.0).round() / 10.0;
                let r6 = f(r, "ret_6m_pct").or_else(|| f(r, "ret_3m_pct")).unwrap_or(0.0);
                let vol = f(r, "vol_60d_pct").unwrap_or(99.0);
                let matched = above && r6 >= 8.0 && vol <= 48.0;
                let mut reasons = Vec::new();
                if let Some(v) = f(r, "ret_6m_pct") {
                    reasons.push(format!("최근 6개월 {v:+.1}%"));
                } else if let Some(v) = f(r, "ret_3m_pct") {
                    reasons.push(format!("최근 3개월 {v:+.1}%"));
                }
                if above { reasons.push("200일 평균선 위".to_string()); }
                if let Some(v) = f(r, "near_52w_pct") {
                    if v >= 90.0 { reasons.push(format!("52주 최고가의 {v:.0}%")); }
                }
                if let Some(v) = f(r, "vol_60d_pct") {
                    if v <= 30.0 { reasons.push(format!("낮은 출렁임(연 {v:.0}%)")); }
                }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 상승 추세 충족" } else { "근접 후보" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
        "oversold_bounce" => {
            let p_rsi = pct_ranks(&rows.iter().map(|r| f(r, "rsi2")).collect::<Vec<_>>(), false);
            let p_ibs = pct_ranks(&rows.iter().map(|r| f(r, "ibs")).collect::<Vec<_>>(), false);
            let p_1m = pct_ranks(&rows.iter().map(|r| f(r, "ret_1m_pct")).collect::<Vec<_>>(), false);
            for (i, r) in rows.iter_mut().enumerate() {
                let base = blend(&[(p_rsi[i], 0.50), (p_ibs[i], 0.35), (p_1m[i], 0.15)]).unwrap_or(0.0);
                let above = b(r, "above_ma200");
                let sc = ((base + if above { 15.0 } else { -22.0 }).clamp(0.0, 100.0) * 10.0).round() / 10.0;
                let rsi_ok = f(r, "rsi2").is_some_and(|v| v <= 25.0);
                let ibs_ok = f(r, "ibs").is_some_and(|v| v <= 0.25);
                let matched = above && (rsi_ok || ibs_ok);
                let mut reasons = vec![if above { "장기 상승(200일선 위)" } else { "주의: 200일선 아래" }.to_string()];
                if let Some(v) = f(r, "rsi2") {
                    let tag = if v <= 20.0 { "과매도 바닥권" } else if v <= 35.0 { "단기 눌림" } else { "중립" };
                    reasons.push(format!("RSI(2) {v:.1} ({tag})"));
                }
                if let Some(v) = f(r, "ibs") {
                    reasons.push(format!("하루 위치(IBS) {v:.2}"));
                }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 과매도 반등 충족" } else { "근접 후보" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
        "smart_money" => {
            let p_f5 = pct_ranks(&rows.iter().map(|r| f(r, "flow_5d_for")).collect::<Vec<_>>(), true);
            let p_i5 = pct_ranks(&rows.iter().map(|r| f(r, "flow_5d_inst")).collect::<Vec<_>>(), true);
            let p_f20 = pct_ranks(&rows.iter().map(|r| f(r, "flow_20d_for")).collect::<Vec<_>>(), true);
            let p_i20 = pct_ranks(&rows.iter().map(|r| f(r, "flow_20d_inst")).collect::<Vec<_>>(), true);
            let p_tr = pct_ranks(&rows.iter().map(|r| f(r, "ret_3m_pct")).collect::<Vec<_>>(), true);
            for (i, r) in rows.iter_mut().enumerate() {
                let base = blend(&[(p_f5[i], 0.30), (p_i5[i], 0.30), (p_f20[i], 0.15), (p_i20[i], 0.15), (p_tr[i], 0.10)]).unwrap_or(0.0);
                let f5 = f(r, "flow_5d_for").unwrap_or(0.0);
                let i5 = f(r, "flow_5d_inst").unwrap_or(0.0);
                let f20 = f(r, "flow_20d_for").unwrap_or(0.0);
                let i20 = f(r, "flow_20d_inst").unwrap_or(0.0);
                let both_5 = f5 > 0.0 && i5 > 0.0;
                let both_20 = f20 > 0.0 && i20 > 0.0;
                let bonus = (if both_5 { 14.0 } else { 0.0 }) + (if both_20 { 8.0 } else { 0.0 });
                let sc = ((base + bonus).clamp(0.0, 100.0) * 10.0).round() / 10.0;
                let matched = both_5 || both_20;
                let mut reasons = Vec::new();
                if both_5 { reasons.push("최근 5일 외국인·기관 동시 순매수".to_string()); }
                else if f5 > 0.0 { reasons.push("최근 5일 외국인 순매수".to_string()); }
                else if i5 > 0.0 { reasons.push("최근 5일 기관 순매수".to_string()); }
                if both_20 { reasons.push("20일 누적 쌍끌이 매집".to_string()); }
                if let Some(fd) = f(r, "foreign_delta_pct").filter(|&x| x > 0.0) {
                    reasons.push(format!("외국인 지분 {fd:+.2}%p 증가"));
                }
                if reasons.is_empty() { reasons.push("최근 수급 관망세".to_string()); }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 외인·기관 쌍끌이" } else { "단독 수급/관망" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
        "strategy_fit" => {
            let p_sh = pct_ranks(&rows.iter().map(|r| f(r, "fit_sharpe")).collect::<Vec<_>>(), true);
            let p_ret = pct_ranks(&rows.iter().map(|r| f(r, "fit_return_pct")).collect::<Vec<_>>(), true);
            let p_mdd = pct_ranks(&rows.iter().map(|r| f(r, "fit_mdd_pct").map(|x| x.abs())).collect::<Vec<_>>(), false);
            let p_win = pct_ranks(&rows.iter().map(|r| f(r, "fit_win_rate")).collect::<Vec<_>>(), true);
            for (i, r) in rows.iter_mut().enumerate() {
                let base = blend(&[(p_sh[i], 0.40), (p_ret[i], 0.30), (p_mdd[i], 0.15), (p_win[i], 0.15)]).unwrap_or(0.0);
                let trades = r.get("fit_trades").and_then(|v| v.as_u64()).unwrap_or(0);
                let pen = if trades < 2 { -25.0 } else if trades < 4 { -10.0 } else { 0.0 };
                let sc = ((base + pen).clamp(0.0, 100.0) * 10.0).round() / 10.0;
                let matched = trades >= 3 && f(r, "fit_return_pct").unwrap_or(0.0) > 0.0 && f(r, "fit_sharpe").unwrap_or(0.0) >= 0.5;
                let mut reasons = Vec::new();
                if let Some(v) = f(r, "fit_return_pct") { reasons.push(format!("최근 2년 전략 수익 {v:+.1}%")); }
                if let Some(v) = f(r, "fit_win_rate") { reasons.push(format!("승률 {v:.0}% ({trades}회 매매)")); }
                if let Some(v) = f(r, "fit_mdd_pct") { reasons.push(format!("최대 낙폭 {v:.1}%")); }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 전략 궁합 우수" } else { "근접 후보" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
        "magic_formula" => {
            let p_roe = pct_ranks(&rows.iter().map(|r| f(r, "roe").filter(|&v| v > 0.0)).collect::<Vec<_>>(), true);
            let p_per = pct_ranks(&rows.iter().map(|r| f(r, "per").filter(|&v| v > 0.0)).collect::<Vec<_>>(), false);
            for (i, r) in rows.iter_mut().enumerate() {
                let (sc, matched) = match (p_roe[i], p_per[i]) {
                    (Some(ro), Some(pe)) => {
                        let s = (((ro + pe) / 2.0) * 10.0).round() / 10.0;
                        let m = f(r, "per").is_some_and(|v| v > 0.0 && v <= 20.0)
                            && f(r, "roe").is_some_and(|v| v >= 12.0);
                        (s, m)
                    }
                    _ => {
                        let s = ((blend(&[(p_roe[i], 0.5), (p_per[i], 0.5)]).unwrap_or(0.0) * 0.4) * 10.0).round() / 10.0;
                        (s, false)
                    }
                };
                let mut reasons = Vec::new();
                if let Some(v) = f(r, "roe") {
                    reasons.push(format!("ROE {v:.1}% ({})", if v >= 15.0 { "돈 잘 버는 우량주" } else { "수익성 양호" }));
                }
                if let Some(v) = f(r, "per") {
                    reasons.push(format!("PER {v:.1}배 ({})", if v <= 15.0 { "저평가 구간" } else { "이익 대비 가격" }));
                }
                if let Some(v) = f(r, "pbr") { reasons.push(format!("PBR {v:.2}배")); }
                if reasons.is_empty() { reasons.push("재무 지표 없음 (개별 기업 묶음 권장)".to_string()); }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 마법공식 우량" } else { "근접 후보" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
        "dividend_lowvol" => {
            let p_div = pct_ranks(&rows.iter().map(|r| f(r, "dividend_yield")).collect::<Vec<_>>(), true);
            let p_vol = pct_ranks(&rows.iter().map(|r| f(r, "vol_60d_pct")).collect::<Vec<_>>(), false);
            let p_tr = pct_ranks(&rows.iter().map(|r| f(r, "ret_12m_pct").or_else(|| f(r, "ret_6m_pct"))).collect::<Vec<_>>(), true);
            for (i, r) in rows.iter_mut().enumerate() {
                let base = blend(&[(p_div[i], 0.50), (p_vol[i], 0.35), (p_tr[i], 0.15)]).unwrap_or(0.0);
                let div = f(r, "dividend_yield").unwrap_or(0.0);
                let pen = if div <= 0.0 { -20.0 } else { 0.0 };
                let sc = ((base + pen).clamp(0.0, 100.0) * 10.0).round() / 10.0;
                let vol = f(r, "vol_60d_pct").unwrap_or(99.0);
                let matched = div >= 2.5 && vol <= 35.0;
                let mut reasons = Vec::new();
                if div > 0.0 { reasons.push(format!("연 배당률 {div:.2}%")); }
                if let Some(v) = f(r, "vol_60d_pct") { reasons.push(format!("주가 출렁임 연 {v:.0}%")); }
                if b(r, "above_ma200") { reasons.push("장기 흐름 안정 (200일선 위)".to_string()); }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 고배당·저변동 충족" } else { "근접 후보" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
        _ => {
            // multifactor
            let p_per = pct_ranks(&rows.iter().map(|r| f(r, "per").filter(|&v| v > 0.0)).collect::<Vec<_>>(), false);
            let p_pbr = pct_ranks(&rows.iter().map(|r| f(r, "pbr").filter(|&v| v > 0.0)).collect::<Vec<_>>(), false);
            let p_roe = pct_ranks(&rows.iter().map(|r| f(r, "roe").filter(|&v| v > 0.0)).collect::<Vec<_>>(), true);
            let p_m6 = pct_ranks(&rows.iter().map(|r| f(r, "ret_6m_pct")).collect::<Vec<_>>(), true);
            let p_m12 = pct_ranks(&rows.iter().map(|r| f(r, "ret_12m_pct")).collect::<Vec<_>>(), true);
            let p_vol = pct_ranks(&rows.iter().map(|r| f(r, "vol_60d_pct")).collect::<Vec<_>>(), false);
            for (i, r) in rows.iter_mut().enumerate() {
                let val_s = blend(&[(p_per[i], 0.55), (p_pbr[i], 0.45)]);
                let qual_s = p_roe[i];
                let mom_s = blend(&[(p_m6[i], 0.45), (p_m12[i], 0.35), (p_vol[i], 0.20)]);
                let sc = (blend(&[(val_s, 1.0), (qual_s, 1.0), (mom_s, 1.0)]).unwrap_or(0.0) * 10.0).round() / 10.0;
                let matched = f(r, "per").is_some_and(|v| v > 0.0)
                    && f(r, "roe").is_some_and(|v| v >= 10.0)
                    && (b(r, "above_ma200") || f(r, "ret_6m_pct").unwrap_or(0.0) > 0.0)
                    && sc >= 60.0;
                let mut reasons = Vec::new();
                if val_s.is_some_and(|v| v >= 60.0) {
                    if let Some(p) = f(r, "per") { reasons.push(format!("저평가(PER {p:.1}배)")); }
                }
                if qual_s.is_some_and(|v| v >= 60.0) {
                    if let Some(ro) = f(r, "roe") { reasons.push(format!("우량 수익성(ROE {ro:.1}%)")); }
                }
                if mom_s.is_some_and(|v| v >= 60.0) {
                    if let Some(m6) = f(r, "ret_6m_pct") { reasons.push(format!("상승 추세(6개월 {m6:+.1}%)")); }
                }
                if reasons.is_empty() {
                    if let (Some(p), Some(ro)) = (f(r, "per"), f(r, "roe")) {
                        reasons.push(format!("PER {p:.1}배 · ROE {ro:.1}%"));
                    } else if let Some(m6) = f(r, "ret_6m_pct") {
                        reasons.push(format!("6개월 {m6:+.1}%"));
                    }
                }
                reasons.truncate(3);
                let obj = r.as_object_mut().unwrap();
                obj.insert("score".to_string(), json!(sc));
                obj.insert("matched".to_string(), json!(matched));
                obj.insert("match_label".to_string(), json!(if matched { "★ 3박자 우량 충족" } else { "근접 후보" }));
                obj.insert("reasons".to_string(), json!(reasons));
            }
        }
    }

    rows.sort_by(|a, b_val| {
        let ma = if b(a, "matched") { 1 } else { 0 };
        let mb = if b(b_val, "matched") { 1 } else { 0 };
        let sa = f(a, "score").unwrap_or(0.0);
        let sb = f(b_val, "score").unwrap_or(0.0);
        let ra = f(a, "ret_6m_pct").unwrap_or(-9999.0);
        let rb = f(b_val, "ret_6m_pct").unwrap_or(-9999.0);
        mb.cmp(&ma).then_with(|| sb.total_cmp(&sa)).then_with(|| rb.total_cmp(&ra))
    });
    for (idx, r) in rows.iter_mut().enumerate() {
        r.as_object_mut().unwrap().insert("rank".to_string(), json!(idx + 1));
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_screen(
    state: &AppState,
    user_id: &str,
    market: Market,
    preset_id: &str,
    universe_id: &str,
    custom_symbols: Option<&[String]>,
    strategy_id: Option<&str>,
    exclude_warnings: bool,
    strict_only: bool,
    top_n: usize,
) -> ApiResult<Value> {
    let preset = get_preset(preset_id)
        .ok_or_else(|| ApiError::bad_request("unknown_preset", "알 수 없는 종목 발굴 방식입니다"))?;
    if preset.kr_only && market != Market::KR {
        return Err(ApiError::bad_request(
            "kr_only_preset",
            "외국인·기관 수급 발굴은 국내(KR) 시장에서만 제공됩니다. 국내 탭으로 전환해 주세요",
        ));
    }
    let fit_spec: Option<&'static StrategySpec> = if preset.uses_strategy {
        let sid = strategy_id.unwrap_or(preset.recommended_strategy);
        let spec = get_spec(sid)
            .ok_or_else(|| ApiError::bad_request("unknown_strategy", "알 수 없는 전략입니다"))?;
        if spec.timeframe != "1d" {
            return Err(ApiError::bad_request(
                "daily_strategy_only",
                "전략 궁합 발굴은 일봉 전략(스윙·일봉 단타)만 지원합니다",
            ));
        }
        Some(spec)
    } else {
        None
    };

    let is_mock = market::provider_name(state, market) != "toss";
    let (uni_norm, candidates) =
        resolve_candidates(state, market, universe_id, custom_symbols, is_mock, user_id).await?;
    let syms: Vec<String> = candidates.iter().map(|c| c.0.clone()).collect();
    let name_map: HashMap<String, String> = candidates.iter().cloned().collect();

    let d = cost_defaults(state, user_id, market).await;
    let costs = Costs {
        commission_pct: d["commission_pct"].as_f64().unwrap_or(0.015),
        tax_pct: d["tax_pct"].as_f64().unwrap_or(0.0),
        slippage_pct: d["slippage_pct"].as_f64().unwrap_or(0.05),
    };
    let capital = d["capital"].as_f64().unwrap_or(10_000_000.0);
    let t0 = Instant::now();

    let mut bars_map: HashMap<String, Bars> = HashMap::new();
    let data_source: &str;
    if is_mock {
        let fetch_from = Utc::now().timestamp() - if fit_spec.is_some() { 850 } else { 480 } * 86400;
        for sym in &syms {
            if let Ok(candles) =
                market::get_history(state, user_id, market, sym, Interval::D1, fetch_from).await
            {
                let b = Bars::from_candles(&candles);
                if b.len() >= 40 {
                    bars_map.insert(sym.clone(), b);
                }
            }
        }
        data_source = "연습용 가상 데이터";
    } else {
        let bar_count = if fit_spec.is_some() { 520 } else { 320 };
        bars_map = sdata::fetch_public_bars(market.as_str(), &syms, bar_count).await;
        let missing: Vec<String> = syms.iter().filter(|s| !bars_map.contains_key(*s)).take(5).cloned().collect();
        if !missing.is_empty() {
            let fetch_from = Utc::now().timestamp() - if fit_spec.is_some() { 850 } else { 480 } * 86400;
            for sym in missing {
                if let Ok(candles) =
                    market::get_history(state, user_id, market, &sym, Interval::D1, fetch_from).await
                {
                    let b = Bars::from_candles(&candles);
                    if b.len() >= 40 {
                        bars_map.insert(sym, b);
                    }
                }
            }
        }
        data_source = if market == Market::KR {
            "네이버 금융 공개 시세"
        } else {
            "Yahoo Finance 공개 시세"
        };
    }

    let valid_syms: Vec<String> = syms
        .iter()
        .filter(|s| bars_map.get(*s).is_some_and(|b| b.len() >= 40 && *b.close.last().unwrap_or(&0.0) > 0.0))
        .cloned()
        .collect();
    let skipped = syms.len() - valid_syms.len();

    let fundamentals_map = if preset.needs_fundamentals {
        fundamentals::get_fundamentals(market.as_str(), &valid_syms, is_mock).await
    } else {
        HashMap::new()
    };
    let fundamentals_source: Option<String> = if preset.needs_fundamentals {
        fundamentals_map
            .values()
            .filter_map(|v| v.get("source").and_then(|s| s.as_str()))
            .find(|&s| s != "unavailable")
            .map(|s| s.to_string())
            .or_else(|| Some("unavailable".to_string()))
    } else {
        None
    };

    let flows_map = if preset_id == "smart_money" {
        if is_mock {
            valid_syms.iter().map(|s| (s.clone(), mock_flow(s))).collect()
        } else {
            sdata::fetch_kr_flows(&valid_syms).await
        }
    } else {
        HashMap::new()
    };

    // Parallel indicator + strategy-fit computation via Rayon
    let mut rows: Vec<Value> = valid_syms
        .par_iter()
        .map(|sym| {
            let bars = &bars_map[sym];
            let mut row = compute_row_stats(bars, fit_spec, capital, costs);
            let f = fundamentals_map.get(sym);
            let obj = row.as_object_mut().unwrap();
            obj.insert("symbol".to_string(), json!(sym));
            obj.insert("name".to_string(), json!(name_map.get(sym).unwrap_or(sym)));
            obj.insert("per".to_string(), f.and_then(|x| x.get("per")).cloned().unwrap_or(Value::Null));
            obj.insert("pbr".to_string(), f.and_then(|x| x.get("pbr")).cloned().unwrap_or(Value::Null));
            obj.insert("roe".to_string(), f.and_then(|x| x.get("roe")).cloned().unwrap_or(Value::Null));
            obj.insert("dividend_yield".to_string(), f.and_then(|x| x.get("dividend_yield")).cloned().unwrap_or(Value::Null));
            obj.insert("market_cap".to_string(), f.and_then(|x| x.get("market_cap")).cloned().unwrap_or(Value::Null));
            obj.insert("warnings".to_string(), json!([]));
            obj.insert("matched".to_string(), json!(false));
            obj.insert("match_label".to_string(), json!(""));
            obj.insert("score".to_string(), json!(0.0));
            obj.insert("reasons".to_string(), json!([]));
            if preset_id == "smart_money" {
                let fl = flows_map.get(sym);
                for k in ["flow_5d_for", "flow_5d_inst", "flow_20d_for", "flow_20d_inst", "foreign_delta_pct"] {
                    obj.insert(k.to_string(), fl.and_then(|x| x.get(k)).cloned().unwrap_or(Value::Null));
                }
            }
            row
        })
        .collect();

    score_and_explain(preset_id, &mut rows);
    if strict_only {
        rows.retain(|r| r.get("matched").and_then(|v| v.as_bool()).unwrap_or(false));
    }

    let limit = top_n.clamp(1, 40);
    let mut excluded_warnings: Vec<String> = Vec::new();
    let mut final_items: Vec<Value> = Vec::new();
    let check_cap = limit + 10;
    let mut checked = 0usize;

    for mut r in rows.iter().cloned() {
        if final_items.len() >= limit {
            break;
        }
        if !is_mock && market == Market::KR && checked < check_cap {
            checked += 1;
            let sym = r["symbol"].as_str().unwrap_or("").to_string();
            if let Ok(Value::Array(ws)) = extras::warnings(state, user_id, market, &sym).await {
                let labels: Vec<String> = ws
                    .iter()
                    .filter_map(|w| w.get("label").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect();
                r.as_object_mut().unwrap().insert("warnings".to_string(), json!(labels));
                if exclude_warnings && !labels.is_empty() {
                    excluded_warnings.push(sym);
                    continue;
                }
            }
        }
        final_items.push(r);
    }

    for (idx, r) in final_items.iter_mut().enumerate() {
        r.as_object_mut().unwrap().insert("rank".to_string(), json!(idx + 1));
    }

    let matched_total = rows
        .iter()
        .filter(|r| {
            r.get("matched").and_then(|v| v.as_bool()).unwrap_or(false)
                && !excluded_warnings
                    .iter()
                    .any(|s| Some(s.as_str()) == r.get("symbol").and_then(|v| v.as_str()))
        })
        .count();

    let mut notices = Vec::new();
    if is_mock {
        notices.push("현재 연습용 가상 데이터로 발굴 결과를 보여주고 있습니다. [설정]에서 토스증권 API 키를 연결하면 실제 시장 데이터로 스캔합니다.");
    }
    if preset.needs_fundamentals && (uni_norm == "kr_etf" || uni_norm == "us_etf") {
        notices.push("ETF는 기업 재무제표(PER·PBR·ROE)가 없어 재무 점수가 비어 있을 수 있습니다. 가치·마법공식 발굴은 개별 기업 묶음을 권장합니다.");
    }
    if preset.needs_fundamentals && fundamentals_source.as_deref() == Some("unavailable") {
        notices.push("외부 재무 데이터 서버 응답이 지연되어 가격 지표 위주로 계산되었습니다. 잠시 후 다시 시도해 주세요.");
    }
    if skipped > 0 && rows.is_empty() {
        notices.push("시세 데이터를 불러오지 못한 종목이 많아 결과가 비어 있습니다. 잠시 후 다시 시도해 주세요.");
    }
    if strict_only && final_items.is_empty() {
        notices.push("오늘 시장에서 엄격한 조건을 완벽히 충족한 종목이 없습니다. '조건 완벽 충족 종목만 보기' 체크를 풀면 가장 근접한 상위 후보를 볼 수 있습니다.");
    }

    let elapsed = (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    Ok(json!({
        "market": market.as_str(),
        "currency": if market == Market::KR { "KRW" } else { "USD" },
        "preset": preset_id,
        "universe": uni_norm,
        "strategy": fit_spec.map(|s| s.id).unwrap_or(preset.recommended_strategy),
        "recommended_portfolio": preset.recommended_portfolio,
        "scanned": candidates.len(),
        "passed": rows.len(),
        "matched_count": matched_total,
        "skipped": skipped,
        "excluded_warnings": excluded_warnings,
        "data_source": data_source,
        "fundamentals_source": fundamentals_source,
        "elapsed_sec": elapsed,
        "notices": notices,
        "items": final_items,
    }))
}
