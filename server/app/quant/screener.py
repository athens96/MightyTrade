"""Quant Stock Screener: large-universe (100~200 stocks) quant extraction using fast public
daily bars, KR foreign+institution flows, strategy-fit backtesting, and free fundamentals
(Magic Formula, High-Dividend Low-Vol, Multi-Factor) with Toss warning safety filtering.
"""
from __future__ import annotations

import hashlib
import math
import time
from typing import Any, Optional

from flask import current_app, g

from ..errors import ApiError
from ..market import extras
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..storage import get_store
from ..users.routes import DEFAULT_WATCHLIST, WATCHLISTS
from . import fundamentals as fund_mod
from . import indicators as ind
from . import screener_data as sdata
from .data import cost_defaults
from .engine import Backtester, Bars, Costs
from .portfolio import SYMBOL_RE
from .strategies import STRATEGIES

MAX_CUSTOM_SYMBOLS = 50

# Fallback static KR stocks (used in mock mode or if Naver market-cap list is unreachable)
KR_FALLBACK_KOSPI: list[tuple[str, str]] = [
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
]

KR_FALLBACK_KOSDAQ: list[tuple[str, str]] = [
    ("196170", "알테오젠"), ("247540", "에코프로비엠"), ("086520", "에코프로"),
    ("028300", "HLB"), ("277810", "레인보우로보틱스"), ("058470", "리노공업"),
    ("214150", "클래시스"), ("039030", "이오테크닉스"), ("035900", "JYP Ent."),
    ("041510", "에스엠"), ("257720", "실리콘투"), ("293490", "카카오게임즈"),
    ("112040", "위메이드"), ("067160", "SOOP"), ("145020", "휴젤"),
]

KR_ETF_LIST: list[tuple[str, str]] = [
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
]

# 100 liquid S&P 500 leaders spanning all 11 GICS sectors (growth + value + dividend + defensive)
US_SP100_LIST: list[tuple[str, str]] = [
    # Tech & Semiconductors
    ("AAPL", "Apple"), ("MSFT", "Microsoft"), ("NVDA", "NVIDIA"), ("AVGO", "Broadcom"),
    ("ORCL", "Oracle"), ("CRM", "Salesforce"), ("ADBE", "Adobe"), ("CSCO", "Cisco"),
    ("ACN", "Accenture"), ("IBM", "IBM"), ("QCOM", "Qualcomm"), ("TXN", "Texas Instruments"),
    ("AMD", "AMD"), ("AMAT", "Applied Materials"), ("INTU", "Intuit"), ("NOW", "ServiceNow"),
    ("MU", "Micron"), ("LRCX", "Lam Research"), ("ADI", "Analog Devices"), ("KLAC", "KLA"),
    # Communication Services
    ("GOOGL", "Alphabet"), ("META", "Meta"), ("NFLX", "Netflix"), ("DIS", "Disney"),
    ("TMUS", "T-Mobile"), ("VZ", "Verizon"), ("T", "AT&T"), ("CMCSA", "Comcast"),
    # Consumer Discretionary
    ("AMZN", "Amazon"), ("TSLA", "Tesla"), ("HD", "Home Depot"), ("MCD", "McDonald's"),
    ("LOW", "Lowe's"), ("BKNG", "Booking"), ("TJX", "TJX"), ("NKE", "Nike"),
    ("SBUX", "Starbucks"), ("GM", "General Motors"), ("F", "Ford"),
    # Consumer Staples
    ("WMT", "Walmart"), ("COST", "Costco"), ("PG", "Procter & Gamble"), ("KO", "Coca-Cola"),
    ("PEP", "PepsiCo"), ("PM", "Philip Morris"), ("MO", "Altria"), ("CL", "Colgate-Palmolive"),
    ("MDLZ", "Mondelez"), ("KMB", "Kimberly-Clark"), ("TGT", "Target"),
    # Healthcare
    ("LLY", "Eli Lilly"), ("UNH", "UnitedHealth"), ("JNJ", "Johnson & Johnson"), ("ABBV", "AbbVie"),
    ("MRK", "Merck"), ("TMO", "Thermo Fisher"), ("ABT", "Abbott"), ("ISRG", "Intuitive Surgical"),
    ("AMGN", "Amgen"), ("PFE", "Pfizer"), ("DHR", "Danaher"), ("BMY", "Bristol-Myers"),
    ("GILD", "Gilead"), ("CVS", "CVS Health"), ("MDT", "Medtronic"),
    # Financials
    ("JPM", "JPMorgan"), ("V", "Visa"), ("MA", "Mastercard"), ("BAC", "Bank of America"),
    ("WFC", "Wells Fargo"), ("GS", "Goldman Sachs"), ("MS", "Morgan Stanley"), ("BLK", "BlackRock"),
    ("SCHW", "Charles Schwab"), ("AXP", "American Express"), ("C", "Citigroup"), ("USB", "U.S. Bancorp"),
    ("CB", "Chubb"), ("PGR", "Progressive"), ("MET", "MetLife"),
    # Industrials & Defense
    ("GE", "GE Aerospace"), ("CAT", "Caterpillar"), ("RTX", "RTX"), ("HON", "Honeywell"),
    ("UNP", "Union Pacific"), ("LMT", "Lockheed Martin"), ("DE", "Deere"), ("UPS", "UPS"),
    ("BA", "Boeing"), ("ETN", "Eaton"),
    # Energy
    ("XOM", "Exxon Mobil"), ("CVX", "Chevron"), ("COP", "ConocoPhillips"), ("SLB", "Schlumberger"),
    ("EOG", "EOG Resources"), ("PSX", "Phillips 66"),
    # Utilities, Materials & REITs
    ("NEE", "NextEra Energy"), ("SO", "Southern Co"), ("DUK", "Duke Energy"), ("LIN", "Linde"),
    ("FCX", "Freeport-McMoRan"), ("AMT", "American Tower"), ("PLD", "Prologis"), ("O", "Realty Income"),
]

US_NASDAQ50_LIST: list[tuple[str, str]] = [
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
]

US_DIVIDEND40_LIST: list[tuple[str, str]] = [
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
]

US_ETF_LIST: list[tuple[str, str]] = [
    ("SPY", "미국 S&P500"), ("QQQ", "나스닥100"), ("IWM", "미국 소형주"), ("DIA", "다우존스"),
    ("VTI", "미국 전체주식"), ("VEA", "선진국 주식"), ("VWO", "신흥국 주식"), ("EWY", "한국 MSCI"),
    ("XLK", "기술 (IT)"), ("XLV", "헬스케어"), ("XLF", "금융"), ("XLE", "에너지"),
    ("XLI", "산업재"), ("XLY", "경기소비재"), ("XLP", "필수소비재"), ("XLU", "유틸리티"),
    ("XLRE", "부동산 리츠"), ("XLB", "소재"), ("XLC", "커뮤니케이션"), ("SMH", "반도체"),
    ("SOXX", "필라델피아 반도체"), ("ITA", "미국 방산"), ("XBI", "바이오테크"), ("SCHD", "미국 배당성장"),
    ("VYM", "고배당 주식"), ("JEPI", "커버드콜 인컴"), ("TLT", "미국 장기국채"), ("IEF", "미국 중기국채"),
    ("SHY", "미국 단기국채"), ("GLD", "금"), ("DBC", "원자재"),
]

# Backwards-compatible aliases (old saved localStorage universe IDs)
UNIVERSE_ALIASES = {
    "kr_bluechip": "kr_all_150",
    "us_bluechip": "us_sp100",
}

UNIVERSES: dict[str, dict[str, dict[str, Any]]] = {
    "KR": {
        "kr_all_150": {
            "id": "kr_all_150",
            "name": "국내 코스피·코스닥 핵심 150종",
            "description": "실시간 시가총액 상위 코스피 100 + 코스닥 50 보통주 (우선주·스팩 자동 제외)",
            "count": 150,
            "is_etf": False,
        },
        "kr_kospi_120": {
            "id": "kr_kospi_120",
            "name": "코스피 시총 상위 120종",
            "description": "실시간 코스피 시가총액 상위 대형·중형 보통주 120종목",
            "count": 120,
            "is_etf": False,
        },
        "kr_kosdaq_100": {
            "id": "kr_kosdaq_100",
            "name": "코스닥 시총 상위 100종",
            "description": "실시간 코스닥 시가총액 상위 성장·중소형 보통주 100종목",
            "count": 100,
            "is_etf": False,
        },
        "kr_etf": {
            "id": "kr_etf",
            "name": "국내 대표 섹터·자산 ETF (30종)",
            "description": "지수·미국·반도체·방산·조선·배당·채권·금·원유 대표 ETF 30종목",
            "count": len(KR_ETF_LIST),
            "is_etf": True,
        },
        "watchlist": {
            "id": "watchlist",
            "name": "내 관심종목",
            "description": "시세 화면에서 저장해 둔 내 국내 관심종목 목록",
            "count": 0,
            "is_etf": False,
        },
        "custom": {
            "id": "custom",
            "name": "직접 입력 종목",
            "description": "원하는 종목 코드를 쉼표로 입력해 비교 (최대 50종목)",
            "count": 0,
            "is_etf": False,
        },
    },
    "US": {
        "us_sp100": {
            "id": "us_sp100",
            "name": f"미국 S&P500 핵심 대형주 ({len(US_SP100_LIST)}종)",
            "description": f"11개 전 업종(IT·반도체·헬스케어·금융·에너지·소비재·배당·리츠) 핵심 {len(US_SP100_LIST)}종목",
            "count": len(US_SP100_LIST),
            "is_etf": False,
        },
        "us_nasdaq_50": {
            "id": "us_nasdaq_50",
            "name": "미국 나스닥·AI·성장주 (50종)",
            "description": "반도체·소프트웨어·보안·바이오 등 미국 핵심 성장주 50종목",
            "count": len(US_NASDAQ50_LIST),
            "is_etf": False,
        },
        "us_dividend_40": {
            "id": "us_dividend_40",
            "name": "미국 고배당·배당귀족·가치주 (40종)",
            "description": "꾸준한 배당을 주는 미국 배당귀족·에너지·유틸리티·리츠·금융 40종목",
            "count": len(US_DIVIDEND40_LIST),
            "is_etf": False,
        },
        "us_etf": {
            "id": "us_etf",
            "name": "미국 대표 섹터·자산 ETF (31종)",
            "description": "미국 지수·11개 전 섹터·반도체·방산·배당·국채·금·원자재 대표 ETF 31종목",
            "count": len(US_ETF_LIST),
            "is_etf": True,
        },
        "watchlist": {
            "id": "watchlist",
            "name": "내 관심종목",
            "description": "시세 화면에서 저장해 둔 내 미국 관심종목 목록",
            "count": 0,
            "is_etf": False,
        },
        "custom": {
            "id": "custom",
            "name": "직접 입력 종목",
            "description": "원하는 미국 티커를 쉼표로 입력해 비교 (최대 50종목)",
            "count": 0,
            "is_etf": False,
        },
    },
}


# ------------------------------------------------------------------ 7 beginner-friendly screening presets
SCREENER_PRESETS: dict[str, dict[str, Any]] = {
    p["id"]: p
    for p in (
        {
            "id": "momentum_quality",
            "name": "부드러운 상승 추세 TOP",
            "theory": "추세 모멘텀 + 저변동성 · 롤러코스터 말고 '꾸준히 우상향하는' 주도주 찾기",
            "summary": "최근 3·6·12개월 동안 수익률이 좋으면서도 중간에 덜 출렁인 우량 추세주를 뽑습니다",
            "description": (
                "단순히 급등한 종목은 금방 급락할 위험이 큽니다. 100~150개 종목 중 최근 3·6·12개월 수익률을 하루 출렁임 크기로 나누어 "
                "'마음 편하게 계단식으로 올라가는' 종목에 높은 점수를 주며, 200일 평균선 위와 52주 신고가 근처 종목을 우선 선정합니다."
            ),
            "needs_fundamentals": False,
            "kr_only": False,
            "uses_strategy": False,
            "recommended_strategy": "ts_momentum",
            "recommended_portfolio": "sector_rotation",
            "guide": {
                "background": (
                    "금융 연구에서 '최근 6~12개월 동안 잘 오른 종목이 다음 몇 달도 계속 잘 오르는 경향(모멘텀)'이 전 세계 시장에서 반복 확인되었습니다. "
                    "여기에 '중간에 크게 흔들리지 않을수록 더 좋다(저변동성)'는 조건을 더해, 급등락 테마주 대신 진짜 돈이 몰리며 차분히 오르는 주도주를 찾아냅니다."
                ),
                "setup": [
                    "국내 코스피·코스닥 150종이나 미국 S&P500 100종, 또는 대표 ETF 묶음 모두 잘 맞습니다.",
                    "'조건 완벽 충족 종목만 보기'를 켜면 200일선 위 + 3~6개월 상승 + 적정 변동성을 모두 통과한 종목만 추려 볼 수 있습니다.",
                ],
                "execution": [
                    "후보군(최대 150종목)의 최근 1년치 일봉 가격을 고속으로 불러와 3·6·12개월 수익률과 60일 출렁임(변동성)을 계산합니다.",
                    "'수익률 ÷ 출렁임' 점수와 52주 최고가 근접도, 200일 평균선 위 여부를 합쳐 조건 충족 종목을 우선 정렬합니다.",
                    "뽑힌 종목은 [백테스트]로 검증하거나, 위쪽 [상위 종목 포트폴리오 실험 →]으로 '섹터·종목 모멘텀 로테이션'에 바로 연결하세요.",
                ],
                "caution": "이미 꽤 오른 종목들이 상위에 올라오므로, 한 종목에 몰빵하기보다 상위 3~5종목으로 나누어 담거나 추세 추종 스윙봇과 함께 쓰는 것이 안전합니다.",
            },
        },
        {
            "id": "oversold_bounce",
            "name": "잠깐 너무 빠진 우량주 (반등 후보)",
            "theory": "래리 코너스 RSI(2) + IBS · 장기 상승세 우량주의 일시적 세일 구간 포착",
            "summary": "큰 흐름은 상승(200일선 위)인데 최근 2~5일 사이 과하게 떨어져 반등 가능성이 높은 종목을 찾습니다",
            "description": (
                "장기적으로는 건강하게 오르고 있는 우량주(200일 평균선 위) 중에서, 최근 며칠 사이 투자자들이 겁먹고 과하게 팔아치운 종목을 100~150개 종목에서 골라냅니다. "
                "짧은 기간 과매도 지표인 RSI(2) ≤ 25 또는 하루 위치(IBS) ≤ 0.25에 해당하는 '진짜 세일 구간' 종목을 우선 보여줍니다."
            ),
            "needs_fundamentals": False,
            "kr_only": False,
            "uses_strategy": False,
            "recommended_strategy": "rsi2",
            "recommended_portfolio": "equal_weight",
            "guide": {
                "background": (
                    "건강한 우량주도 시장 뉴스나 단기 차익 실현 때문에 며칠 연속 뚝 떨어지는 '깜짝 세일 구간'이 찾아옵니다. "
                    "단기 역추세의 대가 래리 코너스(RSI 2일)와 하루 봉 위치(IBS) 이론을 결합해, 장기 상승 추세가 살아 있으면서 "
                    "단기적으로만 너무 눌려 곧 고무줄처럼 튀어 오를 가능성이 높은 스윙 반등 후보를 골라냅니다."
                ),
                "setup": [
                    "국내 코스피·코스닥 150종 또는 미국 S&P500 100종처럼 종목 수가 넓은 묶음에서 스캔해야 '진짜 과매도' 종목을 놓치지 않고 찾을 수 있습니다.",
                    "뽑힌 종목은 바로 옆 [백테스트]를 눌러 'RSI(2) 단기 과매도' 또는 'IBS 하루 눌림목' 전략으로 과거 승률을 확인해 보세요.",
                ],
                "execution": [
                    "후보 종목의 200일 평균선 위치(장기 건강 상태), 2일 RSI(최근 이틀간 눌린 정도), IBS(오늘 하루 중 종가 위치)를 계산합니다.",
                    "200일선 위에 있으면서 RSI(2) ≤ 25 또는 IBS ≤ 0.25인 종목에 '★ 조건 충족' 배지를 달아 맨 위에 배치합니다.",
                    "200일선 아래(장기 하락세)에 있는 종목은 '떨어지는 칼날' 위험이 있어 점수를 낮춥니다.",
                ],
                "caution": "기업 자체에 큰 악재(실적 쇼크·유상증자 등)가 터져 급락한 경우엔 반등하지 않고 더 빠질 수 있으므로, 매수 전 [Investing ↗] 링크로 최신 뉴스를 꼭 확인하세요.",
            },
        },
        {
            "id": "smart_money",
            "name": "외국인·기관 쌍끌이 수급 (국내 전용)",
            "theory": "스마트 머니 추종 · 큰손(외국인 + 기관)이 동시에 꾸준히 사 모으는 종목",
            "summary": "최근 5일·20일 동안 외국인과 기관이 함께 순매수하고 외국인 보유 비중이 늘어나는 국내 종목을 추출합니다",
            "description": (
                "국내 주식 시장은 자금력이 큰 외국인과 기관 투자자가 함께 살 때 추세가 이어지는 경우가 많습니다. "
                "코스피·코스닥 100~150종목의 최근 20거래일 일별 투자자 수급을 고속 분석해 외국인·기관 동시 순매수(쌍끌이)와 지분 증가가 뚜렷한 종목을 뽑아냅니다."
            ),
            "needs_fundamentals": False,
            "kr_only": True,
            "uses_strategy": False,
            "recommended_strategy": "ma_cross",
            "recommended_portfolio": "hrp",
            "guide": {
                "background": (
                    "개인 투자자보다 먼저 기업 실적을 탐방하고 거대한 자금을 굴리는 외국인과 기관을 흔히 '스마트 머니(큰손)'라고 부릅니다. "
                    "한쪽만 사는 것보다 외국인과 기관이 '동시에(쌍끌이)' 며칠 이상 꾸준히 사 모으는 종목은 수급이 탄탄해 쉽게 무너지지 않고 우상향할 가능성이 높습니다."
                ),
                "setup": [
                    "국내(KR) 시장 전용 기능입니다. '국내 코스피·코스닥 핵심 150종'이나 '코스피 120종'에서 실행하면 진짜 쌍끌이 주도주를 찾기 좋습니다.",
                    "수급 데이터는 공개 시세 수급(네이버 금융)을 고속 스캔하고, 최종 상위 종목은 토스증권 투자경고 안전필터로 한 번 더 걸러줍니다.",
                ],
                "execution": [
                    "국내 후보 종목의 최근 20거래일 외국인·기관 일별 순매수량과 외국인 보유 비율 변화를 한꺼번에 불러옵니다.",
                    "5일·20일 외국인·기관 동시 순매수(쌍끌이) 종목에 '★ 조건 충족' 배지와 가산점을 부여해 상위에 배치합니다.",
                    "최종 상위 후보 중 토스증권 투자경고·단기과열 종목은 자동 제외됩니다.",
                ],
                "caution": "외국인·기관 수급은 하루아침에 매도로 돌아설 수도 있습니다. 수급만 보고 매수하기보다 차트에서 이미 너무 급등한 자리가 아닌지 꼭 함께 확인하세요.",
            },
        },
        {
            "id": "strategy_fit",
            "name": "전략 궁합 TOP (내 봇과 찰떡 종목)",
            "theory": "전략-종목 궁합 검증 · 선택한 봇 전략이 최근 2년간 가장 잘 먹힌 종목 찾기",
            "summary": "내가 고른 스윙·단타 전략을 후보 종목 전체에 한꺼번에 백테스트해 성적이 제일 좋았던 종목을 뽑습니다",
            "description": (
                "똑같은 전략이라도 종목 성격에 따라 잘 맞는 종목과 안 맞는 종목이 확연히 다릅니다. "
                "추세형 전략은 쭉쭉 뻗는 종목에, 반등형 전략은 박스권 우량주에 잘 맞습니다. 후보 종목 전체에 최근 2년 모의 투자를 돌려 궁합이 최고인 종목을 찾아줍니다."
            ),
            "needs_fundamentals": False,
            "kr_only": False,
            "uses_strategy": True,
            "recommended_strategy": "rsi2",
            "recommended_portfolio": "equal_weight",
            "guide": {
                "background": (
                    "같은 낚싯대라도 바다에 맞는 낚싯대와 민물에 맞는 낚싯대가 다릅니다. "
                    "예를 들어 '터틀·이동평균 교차'는 한 방향으로 길게 움직이는 종목과 궁합이 좋고, '볼린저·RSI(2)'는 오르내림을 반복하는 대형주와 궁합이 좋습니다. "
                    "직접 하나씩 눌러볼 필요 없이, 후보군 전체를 최근 2년치로 자동 실험해 수익률·안정성·승률이 모두 좋았던 찰떡 종목을 골라줍니다."
                ),
                "setup": [
                    "왼쪽에서 테스트해 보고 싶은 일봉 전략(예: RSI(2), 볼린저, 이동평균 교차, 터틀 등)을 고르고 실행하세요.",
                    "마음에 드는 상위 종목의 [백테스트]를 누르면 해당 종목과 전략이 그대로 세팅되어 5년·10년 장기 검증까지 바로 이어갈 수 있습니다.",
                ],
                "execution": [
                    "후보 종목들의 최근 2년치 일봉 데이터를 불러와 선택한 전략의 기본 설정으로 실제 수수료·세금을 반영해 일괄 백테스트합니다.",
                    "매매 횟수 3회 이상 + 수익률 플러스 + 위험 대비 수익(샤프 0.5 이상) 종목을 '★ 조건 충족'으로 우선 배치합니다.",
                    "결과 표에서 각 종목의 최근 2년 수익률·최대 손실폭(MDD)·승률을 한눈에 비교할 수 있습니다.",
                ],
                "caution": "최근 2년 성적이 좋았다고 앞으로도 100% 보장되지는 않습니다. 상위에 뽑힌 종목은 꼭 [워크포워드]나 [몬테카를로] 탭에서 한 번 더 검증한 뒤 봇을 만드세요.",
            },
        },
        {
            "id": "magic_formula",
            "name": "마법공식 (돈 잘 버는 저평가 우량주)",
            "theory": "조엘 그린블라트 · '수익성 좋은 우량 회사(고ROE)'를 '싼 가격(저PER)'에 사기",
            "summary": "이익을 잘 내는 능력(ROE) 순위와 주가가 싼 정도(PER) 순위를 합쳐 최고의 가성비 우량주를 뽑습니다",
            "description": (
                "월스트리트의 전설적인 가치투자자 조엘 그린블라트가 책 '주식 시장을 이기는 작은 책'에서 공개해 전 세계적으로 유명해진 '마법공식'입니다. "
                "100~150개 기업 중 돈을 잘 버는 탄탄한 회사(높은 ROE)인데 가격은 저렴한(낮은 PER) '가성비 최고 우량주'를 자동으로 찾아냅니다."
            ),
            "needs_fundamentals": True,
            "kr_only": False,
            "uses_strategy": False,
            "recommended_strategy": "ma_cross",
            "recommended_portfolio": "equal_weight",
            "guide": {
                "background": (
                    "식당을 고를 때도 '음식은 아주 맛있는데 가격은 저렴한 가성비 맛집'이 최고입니다. 조엘 그린블라트의 마법공식도 똑같습니다. "
                    "① ROE(자기자본이익률 = 회사가 자기 돈으로 1년에 몇 %나 이익을 버는지)가 높을수록 '돈 잘 버는 맛집'이고, "
                    "② PER(주가수익비율 = 지금 주가가 1년 이익의 몇 배인지)이 낮을수록 '가격이 싼 집'입니다. "
                    "두 순위를 더해 가장 앞선 종목들을 골라 투자하면 장기적으로 시장을 크게 앞선다는 것이 수많은 연구로 검증되었습니다."
                ),
                "setup": [
                    "PER·ROE 같은 기업 성적표가 필요하므로 ETF가 아닌 '코스피·코스닥 150종' 또는 '미국 S&P500 100종' 묶음을 선택하세요.",
                    "재무 정보는 공개 시장 데이터(국내 네이버금융 · 미국 야후파이낸스)에서 별도 키 없이 자동으로 가져옵니다.",
                ],
                "execution": [
                    "후보 기업 전체의 최신 PER(저평가 정도)과 ROE(돈 버는 힘)를 불러옵니다. 적자 기업(PER·ROE 0 이하)은 자동 감점됩니다.",
                    "모든 후보에 대해 'ROE 높은 순위'와 'PER 낮은 순위'를 합산해 100점 만점으로 환산하며, PER ≤ 20배 & ROE ≥ 12% 기업을 '★ 조건 충족'으로 표시합니다.",
                    "상위 5~10개 종목을 골라 [상위 종목 포트폴리오 실험 →] 버튼으로 '동일 비중(1/N)' 장기 분산 투자를 해보세요.",
                ],
                "caution": "경기에 따라 일시적으로 이익이 확 늘어난 회사(예: 해운·철강 호황 끝물)는 PER이 착시처럼 낮아 보일 수 있습니다. 1~2종목에 집중하지 말고 최소 5종목 이상 나누어 담으세요.",
            },
        },
        {
            "id": "dividend_lowvol",
            "name": "고배당 + 덜 출렁이는 안정주",
            "theory": "배당 수익 + 저변동성 · 이자처럼 배당 받으면서 마음 편히 들고 갈 종목 찾기",
            "summary": "배당수익률이 높으면서도 평소 주가 흔들림이 적은 안정적인 배당주를 뽑습니다",
            "description": (
                "주가가 롤러코스터처럼 출렁이는 게 싫고, 예금 이자처럼 꾸준한 배당금을 받으며 편안하게 투자하고 싶은 분들을 위한 발굴기입니다. "
                "연 배당률 2.5% 이상이면서 주가 출렁임이 적은(연 32% 이하) 안정적인 배당주를 우선적으로 찾아냅니다."
            ),
            "needs_fundamentals": True,
            "kr_only": False,
            "uses_strategy": False,
            "recommended_strategy": "bollinger",
            "recommended_portfolio": "risk_parity",
            "guide": {
                "background": (
                    "보통 '위험이 커야 수익도 크다'고 생각하지만, 실제 주식 시장 역사에서는 의외로 '덜 출렁이는 주식(저변동성)'이 장기적으로 더 꾸준한 성과를 낸다는 "
                    "'저변동성 이상 현상'이 널리 알려져 있습니다. 여기에 매년 현금으로 들어오는 높은 배당금까지 더해지면, 주가가 잠시 쉬어가는 시기에도 배당으로 버티며 안정적으로 자산을 불릴 수 있습니다."
                ),
                "setup": [
                    "미국은 '미국 고배당·배당귀족·가치주 (40종)' 또는 'S&P500 (100종)', 국내는 '코스피 120종' 묶음을 선택하면 알짜 배당주가 풍성하게 나옵니다.",
                    "배당은 높지만 주가가 계속 폭락 중인 '배당 함정'을 피하기 위해 최근 1년 추세도 함께 참고해 점수를 매깁니다.",
                ],
                "execution": [
                    "후보 종목의 최신 배당수익률(%)과 최근 60일 주가 출렁임(연 변동성 %), 1년 수익률을 함께 불러옵니다.",
                    "연 배당률 2.5% 이상이면서 출렁임이 연 32% 이하인 안정 고배당주를 '★ 조건 충족'으로 맨 위에 배치합니다.",
                    "상위 종목들은 [상위 종목 포트폴리오 실험 →] 버튼으로 '리스크 패리티'나 '동일 비중' 배당 포트폴리오로 바로 구성할 수 있습니다.",
                ],
                "caution": "회사가 이익이 줄었는데도 무리하게 배당을 주거나, 주가가 크게 떨어져서 겉보기에만 배당률이 높아진 종목은 조심해야 합니다. [Investing ↗]에서 배당 이력을 함께 확인하세요.",
            },
        },
        {
            "id": "multifactor",
            "name": "멀티팩터 종합 (가치·우량·상승 3박자)",
            "theory": "파마-프렌치 · 싸고(가치) + 튼튼하고(우량) + 잘 오르는(모멘텀) 만능 우량주",
            "summary": "저평가(PER·PBR) + 돈 잘 버는 힘(ROE) + 최근 상승 추세(모멘텀) 3가지를 골고루 갖춘 종목을 뽑습니다",
            "description": (
                "한 가지 기준만 보면 약점이 생깁니다. 싸기만 한 종목은 계속 안 오를 수 있고, 오르기만 한 종목은 너무 비쌀 수 있습니다. "
                "노벨경제학상 수상 이론(파마-프렌치 팩터 모델)을 바탕으로 ① 저평가(가치) ② 수익성(우량) ③ 상승세(모멘텀) 3박자가 고루 뛰어난 균형 우량주를 뽑습니다."
            ),
            "needs_fundamentals": True,
            "kr_only": False,
            "uses_strategy": False,
            "recommended_strategy": "ts_momentum",
            "recommended_portfolio": "equal_weight",
            "guide": {
                "background": (
                    "노벨경제학상을 받은 유진 파마(Eugene Fama)와 케네스 프렌치의 다요인(멀티팩터) 이론을 쉽게 구현한 종합 채점표입니다. "
                    "① 가치(PER·PBR이 낮아 저렴함) ② 퀄리티(ROE가 높아 돈을 잘 벎) ③ 모멘텀(최근 6~12개월 주가가 실제 상승 중임) — "
                    "이 세 가지 장점은 서로 시기가 엇갈리며 보완해 주기 때문에, 세 점수를 골고루 합치면 '싸면서도 시장이 알아봐 주기 시작한 진짜 우량주'를 찾을 수 있습니다."
                ),
                "setup": [
                    "국내 '코스피·코스닥 핵심 150종' 또는 미국 '미국 S&P500 핵심 대형주 (100종)' 묶음에서 실행하는 것을 가장 추천합니다.",
                    "특정 조건 하나에 치우치지 않고 종합적으로 탄탄한 종목을 고르고 싶을 때 첫 번째로 써보기 좋습니다.",
                ],
                "execution": [
                    "후보 종목의 가격 데이터(6·12개월 추세·변동성)와 재무 데이터(PER·PBR·ROE)를 고속으로 모두 불러옵니다.",
                    "가치 점수(1/3) + 우량 점수(1/3) + 상승 추세 점수(1/3)를 각각 100점 만점으로 매겨 평균을 냅니다.",
                    "뽑힌 상위 5~8개 종목은 장기 분산 포트폴리오로 굴리거나 개별 스윙봇 후보로 활용하세요.",
                ],
                "caution": "재무 지표는 분기 실적 발표 시점에 따라 변동될 수 있으므로, 분기나 반기에 한 번씩 다시 발굴해 포트폴리오를 점검하는 것이 좋습니다.",
            },
        },
    )
}


# ------------------------------------------------------------------ helpers
def _safe_float(v: Optional[float], ndigits: int = 2) -> Optional[float]:
    if v is None or not math.isfinite(v):
        return None
    return round(v, ndigits)


def _pct_ranks(values: list[Optional[float]], *, higher_is_better: bool = True) -> list[Optional[float]]:
    """Map non-None values into [0, 100] percentile scores."""
    valid = [(v, idx) for idx, v in enumerate(values) if v is not None and math.isfinite(v)]
    out: list[Optional[float]] = [None] * len(values)
    if not valid:
        return out
    if len(valid) == 1:
        out[valid[0][1]] = 70.0
        return out
    valid.sort(key=lambda x: x[0], reverse=not higher_is_better)
    denom = len(valid) - 1
    for rank_pos, (_, orig_idx) in enumerate(valid):
        out[orig_idx] = rank_pos / denom * 100.0
    return out


def _blend(scores: list[tuple[Optional[float], float]]) -> Optional[float]:
    num = 0.0
    den = 0.0
    for val, w in scores:
        if val is not None and math.isfinite(val):
            num += val * w
            den += w
    return num / den if den > 0 else None


def _mock_flow(symbol: str) -> dict[str, Any]:
    h = int(hashlib.sha256(f"KR:{symbol}:flow".encode()).hexdigest()[:12], 16)
    f5 = ((h % 900) - 380) * 1000
    i5 = (((h >> 12) % 900) - 380) * 1000
    f20 = f5 * 3 + (((h >> 24) % 500) - 220) * 1000
    i20 = i5 * 3 + (((h >> 32) % 500) - 220) * 1000
    f_delta = round((((h >> 40) % 160) - 70) / 100.0, 2)
    return {
        "flow_5d_for": f5,
        "flow_5d_inst": i5,
        "flow_20d_for": f20,
        "flow_20d_inst": i20,
        "foreign_delta_pct": f_delta,
    }


def _resolve_candidates(
    market: Market,
    universe_id: str,
    custom_symbols: Optional[list[str]],
    *,
    is_mock: bool,
) -> tuple[str, list[tuple[str, str]]]:
    m = market.value
    uid_norm = UNIVERSE_ALIASES.get(universe_id, universe_id)

    if uid_norm == "custom":
        raw = custom_symbols or []
        seen: set[str] = set()
        out: list[tuple[str, str]] = []
        for s in raw:
            sym = str(s or "").strip().upper()
            if not sym or sym in seen:
                continue
            if not SYMBOL_RE[m].fullmatch(sym):
                raise ApiError(400, "invalid_symbol", f"{sym}은(는) 올바른 {m} 종목 코드가 아닙니다")
            seen.add(sym)
            out.append((sym, sym))
            if len(out) >= MAX_CUSTOM_SYMBOLS:
                break
        if len(out) < 2:
            raise ApiError(400, "too_few_symbols", "직접 입력 종목은 최소 2개 이상 입력하세요")
        return uid_norm, out

    if uid_norm == "watchlist":
        rec = get_store().get(WATCHLISTS, g.user["id"])
        wl = rec["items"] if rec else DEFAULT_WATCHLIST
        out = [(it["symbol"], it.get("name") or it["symbol"]) for it in wl if it.get("market") == m]
        if len(out) < 1:
            raise ApiError(400, "empty_watchlist", f"관심종목에 저장된 {m} 종목이 없습니다. 시세 화면에서 종목을 추가하거나 대표 묶음을 선택하세요")
        return uid_norm, out[:MAX_CUSTOM_SYMBOLS]

    if uid_norm not in UNIVERSES[m]:
        raise ApiError(400, "unknown_universe", "알 수 없는 후보 종목 묶음입니다")

    if m == "KR":
        if uid_norm == "kr_etf":
            return uid_norm, list(KR_ETF_LIST)
        if uid_norm == "kr_kospi_120":
            live = [] if is_mock else sdata.fetch_kr_market_leaders("KOSPI", 120)
            return uid_norm, live or list(KR_FALLBACK_KOSPI)
        if uid_norm == "kr_kosdaq_100":
            live = [] if is_mock else sdata.fetch_kr_market_leaders("KOSDAQ", 100)
            return uid_norm, live or list(KR_FALLBACK_KOSDAQ)
        # kr_all_150: top 100 KOSPI + top 50 KOSDAQ
        if is_mock:
            return uid_norm, list(KR_FALLBACK_KOSPI) + list(KR_FALLBACK_KOSDAQ)
        kospi = sdata.fetch_kr_market_leaders("KOSPI", 100) or list(KR_FALLBACK_KOSPI)
        kosdaq = sdata.fetch_kr_market_leaders("KOSDAQ", 50) or list(KR_FALLBACK_KOSDAQ)
        seen_k = {c for c, _ in kospi}
        merged = list(kospi) + [(c, n) for c, n in kosdaq if c not in seen_k]
        return uid_norm, merged

    # US static curated large universes
    if uid_norm == "us_sp100":
        return uid_norm, list(US_SP100_LIST)
    if uid_norm == "us_nasdaq_50":
        return uid_norm, list(US_NASDAQ50_LIST)
    if uid_norm == "us_dividend_40":
        return uid_norm, list(US_DIVIDEND40_LIST)
    return uid_norm, list(US_ETF_LIST)


def presets_meta() -> dict[str, Any]:
    return {
        "presets": list(SCREENER_PRESETS.values()),
        "universes": {
            m: [
                {"id": u["id"], "name": u["name"], "description": u["description"], "count": u["count"], "is_etf": u["is_etf"]}
                for u in group.values()
            ]
            for m, group in UNIVERSES.items()
        },
        "daily_strategies": [
            {"id": cls.id, "name": cls.name, "category": cls.category}
            for cls in STRATEGIES.values()
            if getattr(cls, "timeframe", "1d") == "1d"
        ],
    }


def _compute_price_stats(bars: Bars) -> dict[str, Any]:
    n = len(bars)
    c = bars.close
    h = bars.high
    lo = bars.low
    v = bars.volume
    last = c[-1]

    def ret(days: int) -> Optional[float]:
        if n <= days or c[-1 - days] <= 0:
            return None
        return (last / c[-1 - days] - 1.0) * 100.0

    vw = min(60, n - 1)
    rets = [c[k] / c[k - 1] - 1.0 for k in range(n - vw, n) if c[k - 1] > 0]
    vol60 = None
    if len(rets) >= 5:
        mu = sum(rets) / len(rets)
        sd = math.sqrt(sum((r - mu) ** 2 for r in rets) / (len(rets) - 1))
        vol60 = sd * math.sqrt(252) * 100.0

    rsi2_series = ind.rsi(c, 2)
    rsi2 = rsi2_series[-1] if rsi2_series else None

    rng = h[-1] - lo[-1]
    ibs = ((last - lo[-1]) / rng) if rng > 0 else 0.5

    ma_win = min(200, n)
    ma200 = sum(c[-ma_win:]) / ma_win if ma_win >= 20 else last
    above_ma200 = last >= ma200

    hi_52w = max(h[-min(252, n) :])
    near_52w = (last / hi_52w * 100.0) if hi_52w > 0 else None

    vol_ratio = None
    if n >= 20:
        v5 = sum(v[-5:]) / 5.0
        v20 = sum(v[-20:]) / 20.0
        if v20 > 0:
            vol_ratio = v5 / v20

    return {
        "price": last,
        "change_1d_pct": ret(1),
        "ret_1m_pct": ret(21),
        "ret_3m_pct": ret(63),
        "ret_6m_pct": ret(126),
        "ret_12m_pct": ret(252),
        "vol_60d_pct": vol60,
        "rsi2": rsi2,
        "ibs": ibs,
        "above_ma200": above_ma200,
        "near_52w_pct": near_52w,
        "vol_ratio_20d": vol_ratio,
    }


def _run_strategy_fit(bars: Bars, strategy_cls: type, capital: float, costs: Costs) -> dict[str, Any]:
    strat = strategy_cls({})
    warm = strat.warmup()
    start = max(warm, len(bars) - 504)
    if start >= len(bars) - 20:
        return {
            "fit_return_pct": None, "fit_cagr_pct": None, "fit_mdd_pct": None,
            "fit_sharpe": None, "fit_win_rate": None, "fit_trades": 0,
        }
    res = Backtester(bars, strat, capital, costs, start).run()
    m = res["metrics"]
    return {
        "fit_return_pct": m.get("total_return_pct"),
        "fit_cagr_pct": m.get("cagr_pct"),
        "fit_mdd_pct": m.get("mdd_pct"),
        "fit_sharpe": m.get("sharpe"),
        "fit_win_rate": m.get("win_rate_pct"),
        "fit_trades": int(m.get("trades") or 0),
    }


def _score_and_explain(preset_id: str, rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if not rows:
        return []

    if preset_id == "momentum_quality":
        raw_mom = []
        for r in rows:
            parts = [x for x in (r["ret_3m_pct"], r["ret_6m_pct"], r["ret_12m_pct"]) if x is not None]
            avg_r = sum(parts) / len(parts) if parts else None
            vol = max(r["vol_60d_pct"] or 20.0, 8.0)
            raw_mom.append(avg_r / vol if avg_r is not None else None)
        p_mom = _pct_ranks(raw_mom, higher_is_better=True)
        p_52 = _pct_ranks([r["near_52w_pct"] for r in rows], higher_is_better=True)
        p_vol = _pct_ranks([r["vol_60d_pct"] for r in rows], higher_is_better=False)
        for i, r in enumerate(rows):
            base = _blend([(p_mom[i], 0.60), (p_52[i], 0.25), (p_vol[i], 0.15)]) or 0.0
            bonus = 8.0 if r["above_ma200"] else -12.0
            r["score"] = round(max(0.0, min(100.0, base + bonus)), 1)
            r6 = r["ret_6m_pct"] if r["ret_6m_pct"] is not None else (r["ret_3m_pct"] or 0.0)
            vol = r["vol_60d_pct"] if r["vol_60d_pct"] is not None else 99.0
            r["matched"] = bool(r["above_ma200"] and r6 >= 8.0 and vol <= 48.0)
            r["match_label"] = "★ 상승 추세 충족" if r["matched"] else "근접 후보"
            reasons = []
            if r["ret_6m_pct"] is not None:
                reasons.append(f"최근 6개월 {r['ret_6m_pct']:+.1f}%")
            elif r["ret_3m_pct"] is not None:
                reasons.append(f"최근 3개월 {r['ret_3m_pct']:+.1f}%")
            if r["above_ma200"]:
                reasons.append("200일 평균선 위")
            if r["near_52w_pct"] is not None and r["near_52w_pct"] >= 90:
                reasons.append(f"52주 최고가의 {r['near_52w_pct']:.0f}%")
            if r["vol_60d_pct"] is not None and r["vol_60d_pct"] <= 30:
                reasons.append(f"낮은 출렁임(연 {r['vol_60d_pct']:.0f}%)")
            r["reasons"] = reasons[:3]

    elif preset_id == "oversold_bounce":
        p_rsi = _pct_ranks([r["rsi2"] for r in rows], higher_is_better=False)
        p_ibs = _pct_ranks([r["ibs"] for r in rows], higher_is_better=False)
        p_1m = _pct_ranks([r["ret_1m_pct"] for r in rows], higher_is_better=False)
        for i, r in enumerate(rows):
            base = _blend([(p_rsi[i], 0.50), (p_ibs[i], 0.35), (p_1m[i], 0.15)]) or 0.0
            trend_adj = 15.0 if r["above_ma200"] else -22.0
            r["score"] = round(max(0.0, min(100.0, base + trend_adj)), 1)
            rsi_ok = r["rsi2"] is not None and r["rsi2"] <= 25.0
            ibs_ok = r["ibs"] is not None and r["ibs"] <= 0.25
            r["matched"] = bool(r["above_ma200"] and (rsi_ok or ibs_ok))
            r["match_label"] = "★ 과매도 반등 충족" if r["matched"] else "근접 후보"
            reasons = []
            reasons.append("장기 상승(200일선 위)" if r["above_ma200"] else "주의: 200일선 아래")
            if r["rsi2"] is not None:
                tag = "과매도 바닥권" if r["rsi2"] <= 20 else "단기 눌림" if r["rsi2"] <= 35 else "중립"
                reasons.append(f"RSI(2) {r['rsi2']:.1f} ({tag})")
            if r["ibs"] is not None:
                reasons.append(f"하루 위치(IBS) {r['ibs']:.2f}")
            r["reasons"] = reasons[:3]

    elif preset_id == "smart_money":
        p_f5 = _pct_ranks([r.get("flow_5d_for") for r in rows], higher_is_better=True)
        p_i5 = _pct_ranks([r.get("flow_5d_inst") for r in rows], higher_is_better=True)
        p_f20 = _pct_ranks([r.get("flow_20d_for") for r in rows], higher_is_better=True)
        p_i20 = _pct_ranks([r.get("flow_20d_inst") for r in rows], higher_is_better=True)
        p_tr = _pct_ranks([r["ret_3m_pct"] for r in rows], higher_is_better=True)
        for i, r in enumerate(rows):
            base = _blend([(p_f5[i], 0.30), (p_i5[i], 0.30), (p_f20[i], 0.15), (p_i20[i], 0.15), (p_tr[i], 0.10)]) or 0.0
            both_5 = (r.get("flow_5d_for") or 0) > 0 and (r.get("flow_5d_inst") or 0) > 0
            both_20 = (r.get("flow_20d_for") or 0) > 0 and (r.get("flow_20d_inst") or 0) > 0
            bonus = (14.0 if both_5 else 0.0) + (8.0 if both_20 else 0.0)
            r["score"] = round(max(0.0, min(100.0, base + bonus)), 1)
            r["matched"] = bool(both_5 or both_20)
            r["match_label"] = "★ 외인·기관 쌍끌이" if r["matched"] else "단독 수급/관망"
            reasons = []
            if both_5:
                reasons.append("최근 5일 외국인·기관 동시 순매수")
            elif (r.get("flow_5d_for") or 0) > 0:
                reasons.append("최근 5일 외국인 순매수")
            elif (r.get("flow_5d_inst") or 0) > 0:
                reasons.append("최근 5일 기관 순매수")
            if both_20:
                reasons.append("20일 누적 쌍끌이 매집")
            if r.get("foreign_delta_pct") is not None and r["foreign_delta_pct"] > 0:
                reasons.append(f"외국인 지분 {r['foreign_delta_pct']:+.2f}%p 증가")
            if not reasons:
                reasons.append("최근 수급 관망세")
            r["reasons"] = reasons[:3]

    elif preset_id == "strategy_fit":
        p_sh = _pct_ranks([r.get("fit_sharpe") for r in rows], higher_is_better=True)
        p_ret = _pct_ranks([r.get("fit_return_pct") for r in rows], higher_is_better=True)
        p_mdd = _pct_ranks([abs(r["fit_mdd_pct"]) if r.get("fit_mdd_pct") is not None else None for r in rows], higher_is_better=False)
        p_win = _pct_ranks([r.get("fit_win_rate") for r in rows], higher_is_better=True)
        for i, r in enumerate(rows):
            base = _blend([(p_sh[i], 0.40), (p_ret[i], 0.30), (p_mdd[i], 0.15), (p_win[i], 0.15)]) or 0.0
            trades = int(r.get("fit_trades") or 0)
            pen = -25.0 if trades < 2 else (-10.0 if trades < 4 else 0.0)
            r["score"] = round(max(0.0, min(100.0, base + pen)), 1)
            r["matched"] = bool(
                trades >= 3
                and (r.get("fit_return_pct") or 0.0) > 0.0
                and (r.get("fit_sharpe") or 0.0) >= 0.5
            )
            r["match_label"] = "★ 전략 궁합 우수" if r["matched"] else "근접 후보"
            reasons = []
            if r.get("fit_return_pct") is not None:
                reasons.append(f"최근 2년 전략 수익 {r['fit_return_pct']:+.1f}%")
            if r.get("fit_win_rate") is not None:
                reasons.append(f"승률 {r['fit_win_rate']:.0f}% ({trades}회 매매)")
            if r.get("fit_mdd_pct") is not None:
                reasons.append(f"최대 낙폭 {r['fit_mdd_pct']:.1f}%")
            r["reasons"] = reasons[:3]

    elif preset_id == "magic_formula":
        p_roe = _pct_ranks([r["roe"] if (r["roe"] is not None and r["roe"] > 0) else None for r in rows], higher_is_better=True)
        p_per = _pct_ranks([r["per"] if (r["per"] is not None and r["per"] > 0) else None for r in rows], higher_is_better=False)
        for i, r in enumerate(rows):
            if p_roe[i] is None or p_per[i] is None:
                r["score"] = round((_blend([(p_roe[i], 0.5), (p_per[i], 0.5)]) or 0.0) * 0.4, 1)
                r["matched"] = False
            else:
                r["score"] = round((p_roe[i] + p_per[i]) / 2.0, 1)
                r["matched"] = bool(r["per"] is not None and 0 < r["per"] <= 20.0 and r["roe"] is not None and r["roe"] >= 12.0)
            r["match_label"] = "★ 마법공식 우량" if r["matched"] else "근접 후보"
            reasons = []
            if r["roe"] is not None:
                reasons.append(f"ROE {r['roe']:.1f}% ({'돈 잘 버는 우량주' if r['roe'] >= 15 else '수익성 양호'})")
            if r["per"] is not None:
                reasons.append(f"PER {r['per']:.1f}배 ({'저평가 구간' if r['per'] <= 15 else '이익 대비 가격'})")
            if r["pbr"] is not None:
                reasons.append(f"PBR {r['pbr']:.2f}배")
            if not reasons:
                reasons.append("재무 지표 없음 (개별 기업 묶음 권장)")
            r["reasons"] = reasons[:3]

    elif preset_id == "dividend_lowvol":
        p_div = _pct_ranks([r["dividend_yield"] for r in rows], higher_is_better=True)
        p_vol = _pct_ranks([r["vol_60d_pct"] for r in rows], higher_is_better=False)
        p_tr = _pct_ranks([r["ret_12m_pct"] if r["ret_12m_pct"] is not None else r["ret_6m_pct"] for r in rows], higher_is_better=True)
        for i, r in enumerate(rows):
            base = _blend([(p_div[i], 0.50), (p_vol[i], 0.35), (p_tr[i], 0.15)]) or 0.0
            pen = -20.0 if (r["dividend_yield"] is None or r["dividend_yield"] <= 0) else 0.0
            r["score"] = round(max(0.0, min(100.0, base + pen)), 1)
            div = r["dividend_yield"] or 0.0
            vol = r["vol_60d_pct"] if r["vol_60d_pct"] is not None else 99.0
            r["matched"] = bool(div >= 2.5 and vol <= 35.0)
            r["match_label"] = "★ 고배당·저변동 충족" if r["matched"] else "근접 후보"
            reasons = []
            if r["dividend_yield"] is not None and r["dividend_yield"] > 0:
                reasons.append(f"연 배당률 {r['dividend_yield']:.2f}%")
            if r["vol_60d_pct"] is not None:
                reasons.append(f"주가 출렁임 연 {r['vol_60d_pct']:.0f}%")
            if r["above_ma200"]:
                reasons.append("장기 흐름 안정 (200일선 위)")
            r["reasons"] = reasons[:3]

    else:  # multifactor
        p_per = _pct_ranks([r["per"] if (r["per"] is not None and r["per"] > 0) else None for r in rows], higher_is_better=False)
        p_pbr = _pct_ranks([r["pbr"] if (r["pbr"] is not None and r["pbr"] > 0) else None for r in rows], higher_is_better=False)
        p_roe = _pct_ranks([r["roe"] if (r["roe"] is not None and r["roe"] > 0) else None for r in rows], higher_is_better=True)
        p_m6 = _pct_ranks([r["ret_6m_pct"] for r in rows], higher_is_better=True)
        p_m12 = _pct_ranks([r["ret_12m_pct"] for r in rows], higher_is_better=True)
        p_vol = _pct_ranks([r["vol_60d_pct"] for r in rows], higher_is_better=False)
        for i, r in enumerate(rows):
            val_s = _blend([(p_per[i], 0.55), (p_pbr[i], 0.45)])
            qual_s = p_roe[i]
            mom_s = _blend([(p_m6[i], 0.45), (p_m12[i], 0.35), (p_vol[i], 0.20)])
            score = _blend([(val_s, 1.0), (qual_s, 1.0), (mom_s, 1.0)]) or 0.0
            r["score"] = round(score, 1)
            r["matched"] = bool(
                (r["per"] is not None and r["per"] > 0)
                and (r["roe"] is not None and r["roe"] >= 10.0)
                and (r["above_ma200"] or (r["ret_6m_pct"] or 0.0) > 0)
                and score >= 60.0
            )
            r["match_label"] = "★ 3박자 우량 충족" if r["matched"] else "근접 후보"
            reasons = []
            if val_s is not None and val_s >= 60 and r["per"] is not None:
                reasons.append(f"저평가(PER {r['per']:.1f}배)")
            if qual_s is not None and qual_s >= 60 and r["roe"] is not None:
                reasons.append(f"우량 수익성(ROE {r['roe']:.1f}%)")
            if mom_s is not None and mom_s >= 60 and r["ret_6m_pct"] is not None:
                reasons.append(f"상승 추세(6개월 {r['ret_6m_pct']:+.1f}%)")
            if not reasons:
                if r["per"] is not None and r["roe"] is not None:
                    reasons.append(f"PER {r['per']:.1f}배 · ROE {r['roe']:.1f}%")
                elif r["ret_6m_pct"] is not None:
                    reasons.append(f"6개월 {r['ret_6m_pct']:+.1f}%")
            r["reasons"] = reasons[:3]

    # Put genuine condition matches first, then sort by score descending
    rows.sort(key=lambda r: (1 if r.get("matched") else 0, r["score"], r.get("ret_6m_pct") or -9999.0), reverse=True)
    for idx, r in enumerate(rows, 1):
        r["rank"] = idx
    return rows


# ------------------------------------------------------------------ main entry
def run_screen(
    market: Market,
    preset_id: str,
    universe_id: str,
    *,
    custom_symbols: Optional[list[str]] = None,
    strategy_id: Optional[str] = None,
    exclude_warnings: bool = True,
    strict_only: bool = False,
    top_n: int = 20,
) -> dict[str, Any]:
    preset = SCREENER_PRESETS.get(preset_id)
    if preset is None:
        raise ApiError(400, "unknown_preset", "알 수 없는 종목 발굴 방식입니다")
    if preset["kr_only"] and market != Market.KR:
        raise ApiError(400, "kr_only_preset", "외국인·기관 수급 발굴은 국내(KR) 시장에서만 제공됩니다. 국내 탭으로 전환해 주세요")

    fit_cls = None
    if preset["uses_strategy"]:
        sid = strategy_id or preset["recommended_strategy"]
        fit_cls = STRATEGIES.get(sid)
        if fit_cls is None:
            raise ApiError(400, "unknown_strategy", "알 수 없는 전략입니다")
        if getattr(fit_cls, "timeframe", "1d") != "1d":
            raise ApiError(400, "daily_strategy_only", "전략 궁합 발굴은 일봉 전략(스윙·일봉 단타)만 지원합니다")

    is_mock = current_app.config["MARKET_PROVIDERS"][market.value] != "toss"
    uni_norm, candidates = _resolve_candidates(market, universe_id, custom_symbols, is_mock=is_mock)
    syms = [s for s, _ in candidates]
    name_map = dict(candidates)

    d = cost_defaults(market)
    costs = Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])
    capital = d["capital"]
    t0 = time.time()

    # 1) Fetch daily OHLCV bars fast without hammering Toss Open API rate limits
    bars_map: dict[str, Bars] = {}
    if is_mock:
        provider = get_provider(market)
        fetch_from = int(time.time() - (850 if fit_cls else 480) * 86400)
        for sym in syms:
            try:
                candles = provider.get_history(market, sym, Interval.D1, fetch_from)
                b = Bars.from_candles(candles)
                if len(b) >= 40:
                    bars_map[sym] = b
            except Exception:
                continue
        data_source = "연습용 가상 데이터"
    else:
        bar_count = 520 if fit_cls else 320
        bars_map = sdata.fetch_public_bars(market.value, syms, count=bar_count)
        # Polite Toss fallback for up to 5 missing custom/exotic tickers
        missing_syms = [s for s in syms if s not in bars_map][:5]
        if missing_syms:
            provider = get_provider(market)
            fetch_from = int(time.time() - (850 if fit_cls else 480) * 86400)
            for sym in missing_syms:
                try:
                    candles = provider.get_history(market, sym, Interval.D1, fetch_from)
                    b = Bars.from_candles(candles)
                    if len(b) >= 40:
                        bars_map[sym] = b
                except Exception:
                    continue
        data_source = "네이버 금융 공개 시세" if market == Market.KR else "Yahoo Finance 공개 시세"

    # 2) Fetch fundamentals if needed (cached 6h)
    fundamentals_map: dict[str, dict[str, Any]] = {}
    fundamentals_source: Optional[str] = None
    if preset["needs_fundamentals"]:
        fundamentals_map = fund_mod.get_fundamentals(market.value, list(bars_map.keys()), use_mock=is_mock)
        sources = {v.get("source") for v in fundamentals_map.values() if v.get("source") and v.get("source") != "unavailable"}
        fundamentals_source = next(iter(sources), "unavailable")

    # 3) Fetch KR foreign+institution flows if needed (fast Naver batch, cached 10m)
    flows_map: dict[str, dict[str, Any]] = {}
    if preset_id == "smart_money":
        if is_mock:
            flows_map = {s: _mock_flow(s) for s in bars_map}
        else:
            flows_map = sdata.fetch_kr_flows(list(bars_map.keys()))

    # 4) Compute indicators & optional strategy-fit per symbol
    rows: list[dict[str, Any]] = []
    skipped = 0
    for sym in syms:
        bars = bars_map.get(sym)
        if bars is None or len(bars) < 40 or bars.close[-1] <= 0:
            skipped += 1
            continue
        stats = _compute_price_stats(bars)
        f = fundamentals_map.get(sym) or {}
        row: dict[str, Any] = {
            "symbol": sym,
            "name": name_map.get(sym, sym),
            "price": round(stats["price"], 2),
            "change_1d_pct": _safe_float(stats["change_1d_pct"]),
            "ret_1m_pct": _safe_float(stats["ret_1m_pct"]),
            "ret_3m_pct": _safe_float(stats["ret_3m_pct"]),
            "ret_6m_pct": _safe_float(stats["ret_6m_pct"]),
            "ret_12m_pct": _safe_float(stats["ret_12m_pct"]),
            "vol_60d_pct": _safe_float(stats["vol_60d_pct"]),
            "rsi2": _safe_float(stats["rsi2"], 1),
            "ibs": _safe_float(stats["ibs"], 2),
            "above_ma200": stats["above_ma200"],
            "near_52w_pct": _safe_float(stats["near_52w_pct"], 1),
            "vol_ratio_20d": _safe_float(stats["vol_ratio_20d"], 2),
            "per": f.get("per"),
            "pbr": f.get("pbr"),
            "roe": f.get("roe"),
            "dividend_yield": f.get("dividend_yield"),
            "market_cap": f.get("market_cap"),
            "warnings": [],
            "matched": False,
            "match_label": "",
            "score": 0.0,
            "reasons": [],
        }
        if preset_id == "smart_money":
            fl = flows_map.get(sym) or {}
            row.update(
                flow_5d_for=fl.get("flow_5d_for"),
                flow_5d_inst=fl.get("flow_5d_inst"),
                flow_20d_for=fl.get("flow_20d_for"),
                flow_20d_inst=fl.get("flow_20d_inst"),
                foreign_delta_pct=fl.get("foreign_delta_pct"),
            )
        if fit_cls is not None:
            row.update(_run_strategy_fit(bars, fit_cls, capital, costs))
        rows.append(row)

    scored = _score_and_explain(preset_id, rows)
    if strict_only:
        scored = [r for r in scored if r.get("matched")]

    # 5) Polite Toss warnings check ONLY on top finalists (never hammers Toss rate limits!)
    limit = max(1, min(int(top_n), 40))
    excluded_warnings: list[str] = []
    final_items: list[dict[str, Any]] = []
    check_cap = limit + 10
    checked = 0
    for r in scored:
        if len(final_items) >= limit:
            break
        if not is_mock and market == Market.KR and checked < check_cap:
            checked += 1
            try:
                wlabels = [w["label"] for w in extras.warnings(market, r["symbol"])]
                r["warnings"] = wlabels
                if exclude_warnings and wlabels:
                    excluded_warnings.append(r["symbol"])
                    continue
            except Exception:
                pass
        final_items.append(r)

    for idx, r in enumerate(final_items, 1):
        r["rank"] = idx

    matched_total = sum(1 for r in scored if r.get("matched") and r["symbol"] not in excluded_warnings)

    notices: list[str] = []
    if is_mock:
        notices.append("현재 연습용 가상 데이터로 발굴 결과를 보여주고 있습니다. [설정]에서 토스증권 API 키를 연결하면 실제 시장 데이터로 스캔합니다.")
    if preset["needs_fundamentals"] and uni_norm in ("kr_etf", "us_etf"):
        notices.append("ETF는 기업 재무제표(PER·PBR·ROE)가 없어 재무 점수가 비어 있을 수 있습니다. 가치·마법공식 발굴은 개별 기업 묶음을 권장합니다.")
    if preset["needs_fundamentals"] and fundamentals_source == "unavailable":
        notices.append("외부 재무 데이터 서버 응답이 지연되어 가격 지표 위주로 계산되었습니다. 잠시 후 다시 시도해 주세요.")
    if skipped > 0 and len(rows) == 0:
        notices.append("시세 데이터를 불러오지 못한 종목이 많아 결과가 비어 있습니다. 잠시 후 다시 시도해 주세요.")
    if strict_only and not final_items:
        notices.append("오늘 시장에서 엄격한 조건을 완벽히 충족한 종목이 없습니다. '조건 완벽 충족 종목만 보기' 체크를 풀면 가장 근접한 상위 후보를 볼 수 있습니다.")

    return {
        "market": market.value,
        "currency": "KRW" if market == Market.KR else "USD",
        "preset": preset_id,
        "universe": uni_norm,
        "strategy": fit_cls.id if fit_cls else preset["recommended_strategy"],
        "recommended_portfolio": preset["recommended_portfolio"],
        "scanned": len(candidates),
        "passed": len(rows),
        "matched_count": matched_total,
        "skipped": skipped,
        "excluded_warnings": excluded_warnings,
        "data_source": data_source,
        "fundamentals_source": fundamentals_source,
        "elapsed_sec": round(time.time() - t0, 2),
        "notices": notices,
        "items": final_items,
    }
