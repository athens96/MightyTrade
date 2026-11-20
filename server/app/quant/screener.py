"""Quant Stock Screener: extract promising stocks using price/trend, oversold bounce,
KR foreign+institution flows, strategy-fit backtesting, and free fundamental metrics
(Magic Formula, High-Dividend Low-Vol, Multi-Factor).
"""
from __future__ import annotations

import hashlib
import math
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Optional

from flask import copy_current_request_context, current_app, g

from ..errors import ApiError
from ..market import extras
from ..market.models import Interval, Market
from ..market.registry import get_provider
from ..storage import get_store
from ..users.routes import DEFAULT_WATCHLIST, WATCHLISTS
from . import fundamentals as fund_mod
from . import indicators as ind
from .data import cost_defaults
from .engine import Backtester, Bars, Costs
from .portfolio import SYMBOL_RE
from .strategies import STRATEGIES

MAX_CUSTOM_SYMBOLS = 30

# ------------------------------------------------------------------ curated liquid universes
UNIVERSES: dict[str, dict[str, dict[str, Any]]] = {
    "KR": {
        "kr_bluechip": {
            "id": "kr_bluechip",
            "name": "코스피 대표 우량주 (25종)",
            "description": "거래량이 풍부한 국내 시가총액 상위 대표 기업 25종목",
            "is_etf": False,
            "items": [
                ("005930", "삼성전자"), ("000660", "SK하이닉스"), ("373220", "LG에너지솔루션"),
                ("207940", "삼성바이오로직스"), ("005380", "현대차"), ("000270", "기아"),
                ("068270", "셀트리온"), ("105560", "KB금융"), ("055550", "신한지주"),
                ("086790", "하나금융지주"), ("005490", "POSCO홀딩스"), ("035420", "NAVER"),
                ("035720", "카카오"), ("006400", "삼성SDI"), ("051910", "LG화학"),
                ("012330", "현대모비스"), ("028260", "삼성물산"), ("033780", "KT&G"),
                ("017670", "SK텔레콤"), ("030200", "KT"), ("015760", "한국전력"),
                ("012450", "한화에어로스페이스"), ("329180", "HD현대중공업"), ("034020", "두산에너빌리티"),
                ("010130", "고려아연"),
            ],
        },
        "kr_etf": {
            "id": "kr_etf",
            "name": "국내 대표 섹터·자산 ETF (16종)",
            "description": "지수·업종·채권·금·배당 등 국내 상장 대표 ETF 16종목",
            "is_etf": True,
            "items": [
                ("069500", "KODEX 200"), ("229200", "코스닥150"), ("360750", "미국 S&P500"),
                ("133690", "미국 나스닥100"), ("091160", "반도체"), ("091180", "자동차"),
                ("091170", "은행"), ("305720", "2차전지산업"), ("244580", "바이오"),
                ("148070", "국고채10년"), ("114260", "국고채3년"), ("153130", "단기채권"),
                ("132030", "골드선물"), ("261220", "WTI원유"), ("458730", "미국배당다우존스"),
                ("161510", "고배당"),
            ],
        },
        "watchlist": {
            "id": "watchlist",
            "name": "내 관심종목",
            "description": "시세 화면에서 저장해 둔 내 국내 관심종목 목록",
            "is_etf": False,
            "items": [],
        },
        "custom": {
            "id": "custom",
            "name": "직접 입력 종목",
            "description": "원하는 종목 코드를 쉼표로 입력해 비교 (최대 30종목)",
            "is_etf": False,
            "items": [],
        },
    },
    "US": {
        "us_bluechip": {
            "id": "us_bluechip",
            "name": "미국 핵심 우량주 (25종)",
            "description": "S&P500·나스닥을 대표하는 글로벌 대형 우량주 25종목",
            "is_etf": False,
            "items": [
                ("AAPL", "Apple"), ("MSFT", "Microsoft"), ("NVDA", "NVIDIA"),
                ("AMZN", "Amazon"), ("GOOGL", "Alphabet"), ("META", "Meta"),
                ("TSLA", "Tesla"), ("AVGO", "Broadcom"), ("JPM", "JPMorgan"),
                ("LLY", "Eli Lilly"), ("V", "Visa"), ("MA", "Mastercard"),
                ("UNH", "UnitedHealth"), ("XOM", "Exxon Mobil"), ("JNJ", "Johnson & Johnson"),
                ("PG", "Procter & Gamble"), ("HD", "Home Depot"), ("COST", "Costco"),
                ("ABBV", "AbbVie"), ("KO", "Coca-Cola"), ("PEP", "PepsiCo"),
                ("MRK", "Merck"), ("CVX", "Chevron"), ("WMT", "Walmart"), ("AMD", "AMD"),
            ],
        },
        "us_etf": {
            "id": "us_etf",
            "name": "미국 대표 섹터·자산 ETF (20종)",
            "description": "미국 지수·8대 업종·채권·금·원자재·배당 대표 ETF 20종목",
            "is_etf": True,
            "items": [
                ("SPY", "미국 S&P500"), ("QQQ", "나스닥100"), ("IWM", "미국 소형주"),
                ("DIA", "다우존스"), ("VEA", "선진국 주식"), ("VWO", "신흥국 주식"),
                ("XLK", "기술 (IT)"), ("XLV", "헬스케어"), ("XLF", "금융"),
                ("XLE", "에너지"), ("XLI", "산업재"), ("XLY", "경기소비재"),
                ("XLP", "필수소비재"), ("XLU", "유틸리티"), ("TLT", "장기국채"),
                ("IEF", "중기국채"), ("SHY", "단기국채"), ("GLD", "금"),
                ("DBC", "원자재"), ("SCHD", "미국 배당성장"),
            ],
        },
        "watchlist": {
            "id": "watchlist",
            "name": "내 관심종목",
            "description": "시세 화면에서 저장해 둔 내 미국 관심종목 목록",
            "is_etf": False,
            "items": [],
        },
        "custom": {
            "id": "custom",
            "name": "직접 입력 종목",
            "description": "원하는 미국 티커를 쉼표로 입력해 비교 (최대 30종목)",
            "is_etf": False,
            "items": [],
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
                "단순히 급등한 종목은 금방 급락할 위험이 큽니다. 최근 3·6·12개월 수익률을 하루 출렁임 크기로 나누어 "
                "'마음 편하게 계단식으로 올라가는' 종목에 높은 점수를 주며, 200일 평균선 위와 52주 신고가 근처 종목을 우대합니다."
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
                    "국내·미국 우량주 묶음이나 대표 ETF 묶음 모두 잘 맞습니다.",
                    "'위험 종목 자동 제외'를 켜두면 국내 시장의 단기과열·투자경고 종목이 자동으로 걸러집니다.",
                ],
                "execution": [
                    "선택한 후보 종목들의 최근 1년치 일봉 가격을 불러와 3·6·12개월 수익률과 최근 60일 출렁임(변동성)을 계산합니다.",
                    "'수익률 ÷ 출렁임' 점수와 52주 최고가 근접도, 200일 평균선 위 여부를 합쳐 100점 만점으로 줄 세웁니다.",
                    "뽑힌 종목은 [백테스트]로 검증하거나, 위쪽 [상위 종목으로 포트폴리오 →] 버튼으로 '섹터·종목 모멘텀 로테이션' 봇에 바로 연결할 수 있습니다.",
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
                "장기적으로는 건강하게 오르고 있는 우량주(200일 평균선 위) 중에서, 최근 며칠 사이 투자자들이 겁먹고 과하게 팔아치운 종목을 찾아냅니다. "
                "짧은 기간의 과매도 지표인 RSI(2)와 하루 종가 위치(IBS)가 바닥권일수록 높은 점수를 받습니다."
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
                    "국내·미국 대표 우량주 또는 대표 지수·섹터 ETF 묶음에서 찾는 것을 추천합니다. (실적이 탄탄한 대형주일수록 반등 확률이 높습니다.)",
                    "뽑힌 종목은 바로 옆 [백테스트]를 눌러 'RSI(2) 단기 과매도' 또는 'IBS 하루 눌림목' 전략으로 과거 승률을 확인해 보세요.",
                ],
                "execution": [
                    "후보 종목의 200일 평균선 위치(장기 건강 상태), 2일 RSI(최근 이틀간 눌린 정도), IBS(오늘 하루 중 종가 위치)를 계산합니다.",
                    "200일 평균선 위에 있으면서 RSI(2)와 IBS가 바닥에 가까울수록 높은 점수를 부여합니다.",
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
                "토스증권의 일별 투자자 수급 데이터를 분석해 최근 5일·20일간 외국인·기관 동반 순매수와 외국인 지분 증가가 뚜렷한 종목을 골라냅니다."
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
                    "국내(KR) 시장 전용 기능입니다. '코스피 대표 우량주' 또는 '내 관심종목'에서 실행해 보세요.",
                    "실제 외국인·기관 수급은 [설정]에서 토스증권 API 키를 연결했을 때 실시간 데이터로 조회됩니다(미연결 시 연습용 데이터).",
                ],
                "execution": [
                    "각 국내 종목의 최근 20거래일 외국인·기관 일별 순매수량과 외국인 보유 비율 변화를 불러옵니다.",
                    "최근 5일·20일 외국인 순매수, 기관 순매수, 양쪽 동시 순매수(쌍끌이) 보너스, 그리고 최근 주가 추세를 합쳐 100점 만점으로 순위를 매깁니다.",
                    "단기과열·투자경고 종목은 위험 필터로 자동 제외됩니다.",
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
                    "후보 종목들의 최근 2년치 일봉 데이터를 불러와 선택한 전략의 기본 설정으로 실제 수수료·세금을 반영해 백테스트합니다.",
                    "위험 대비 수익(샤프 점수), 총수익률, 승률, 그리고 매매 횟수가 너무 적지 않은지를 종합해 100점 만점으로 순위를 매깁니다.",
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
                "돈을 아주 잘 버는 탄탄한 회사인데, 가격은 이익 대비 저렴하게 거래되는 '가성비 최고 우량주'를 자동으로 찾아냅니다."
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
                    "PER·ROE 같은 기업 성적표가 필요하므로 ETF가 아닌 '코스피 대표 우량주' 또는 '미국 핵심 우량주' 묶음을 선택하세요.",
                    "재무 정보는 공개 시장 데이터(국내 네이버금융 · 미국 야후파이낸스)에서 별도 키 없이 자동으로 가져옵니다.",
                ],
                "execution": [
                    "후보 종목의 최신 PER(저평가 정도)과 ROE(돈 버는 힘)를 불러옵니다. 적자 기업(PER·ROE가 0 이하)은 자동 제외됩니다.",
                    "모든 후보에 대해 'ROE 높은 순위'와 'PER 낮은 순위'를 각각 매겨 합산한 뒤 100점 만점으로 환산합니다.",
                    "상위 5~10개 종목을 골라 [상위 종목으로 포트폴리오 →] 버튼으로 '동일 비중(1/N)' 장기 분산 투자를 해보세요.",
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
                "배당수익률이 높을수록, 그리고 최근 60일 주가 출렁임이 작을수록 높은 점수를 받습니다."
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
                    "국내·미국 우량주 묶음 또는 배당 ETF가 포함된 목록에서 실행해 보세요.",
                    "배당은 높지만 주가가 계속 폭락 중인 '배당 함정'을 피하기 위해 최근 1년 추세도 함께 참고해 점수를 매깁니다.",
                ],
                "execution": [
                    "후보 종목의 최신 배당수익률(%)과 최근 60일 주가 출렁임(연 변동성 %), 1년 수익률을 함께 불러옵니다.",
                    "배당수익률이 높을수록(+), 주가 출렁임이 낮을수록(+), 1년 흐름이 무너지지 않았을수록 높은 점수를 줍니다.",
                    "상위 종목들은 [상위 종목으로 포트폴리오 →] 버튼으로 '리스크 패리티'나 '동일 비중' 배당 포트폴리오로 바로 구성할 수 있습니다.",
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
                    "국내 '코스피 대표 우량주' 또는 미국 '미국 핵심 우량주' 묶음에서 실행하는 것을 가장 추천합니다.",
                    "특정 조건 하나에 치우치지 않고 종합적으로 탄탄한 종목을 고르고 싶을 때 첫 번째로 써보기 좋습니다.",
                ],
                "execution": [
                    "후보 종목의 가격 데이터(6·12개월 추세·변동성)와 재무 데이터(PER·PBR·ROE)를 모두 불러옵니다.",
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
    return {"f5": f5, "i5": i5, "f20": f20, "i20": i20, "f_delta": f_delta}


def _resolve_candidates(market: Market, universe_id: str, custom_symbols: Optional[list[str]]) -> list[tuple[str, str]]:
    m = market.value
    if universe_id == "custom":
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
        return out

    if universe_id == "watchlist":
        rec = get_store().get(WATCHLISTS, g.user["id"])
        wl = rec["items"] if rec else DEFAULT_WATCHLIST
        out = [(it["symbol"], it.get("name") or it["symbol"]) for it in wl if it.get("market") == m]
        if len(out) < 1:
            raise ApiError(400, "empty_watchlist", f"관심종목에 저장된 {m} 종목이 없습니다. 시세 화면에서 종목을 추가하거나 대표 묶음을 선택하세요")
        return out[:MAX_CUSTOM_SYMBOLS]

    uni = UNIVERSES[m].get(universe_id)
    if uni is None:
        raise ApiError(400, "unknown_universe", "알 수 없는 후보 종목 묶음입니다")
    return list(uni["items"])


def presets_meta() -> dict[str, Any]:
    return {
        "presets": list(SCREENER_PRESETS.values()),
        "universes": {
            m: [
                {"id": u["id"], "name": u["name"], "description": u["description"], "count": len(u["items"]), "is_etf": u["is_etf"]}
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

    # 60-day annualized volatility (%)
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
    # Evaluate over most recent ~2 years (up to 504 trading days)
    start = max(warm, len(bars) - 504)
    if start >= len(bars) - 20:
        return {"fit_return_pct": None, "fit_cagr_pct": None, "fit_mdd_pct": None,
                "fit_sharpe": None, "fit_win_rate": None, "fit_trades": 0}
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
            reasons = []
            if r["ret_6m_pct"] is not None:
                reasons.append(f"최근 6개월 {r['ret_6m_pct']:+.1f}%")
            elif r["ret_3m_pct"] is not None:
                reasons.append(f"최근 3개월 {r['ret_3m_pct']:+.1f}%")
            if r["above_ma200"]:
                reasons.append("200일 평균선 위 상승 추세")
            if r["near_52w_pct"] is not None and r["near_52w_pct"] >= 90:
                reasons.append(f"52주 최고가의 {r['near_52w_pct']:.0f}% 수준")
            if r["vol_60d_pct"] is not None and r["vol_60d_pct"] <= 25:
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
            reasons = []
            reasons.append("장기 상승 추세(200일선 위)" if r["above_ma200"] else "주의: 200일선 아래")
            if r["rsi2"] is not None:
                tag = "과매도 바닥권" if r["rsi2"] <= 20 else "단기 눌림" if r["rsi2"] <= 40 else "중립"
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
            bonus = (12.0 if both_5 else 0.0) + (6.0 if both_20 else 0.0)
            r["score"] = round(max(0.0, min(100.0, base + bonus)), 1)
            reasons = []
            if both_5:
                reasons.append("최근 5일 외국인·기관 동시 순매수(쌍끌이)")
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
                r["score"] = round(_blend([(p_roe[i], 0.5), (p_per[i], 0.5)]) or 0.0, 1) * 0.5
            else:
                r["score"] = round((p_roe[i] + p_per[i]) / 2.0, 1)
            reasons = []
            if r["roe"] is not None:
                reasons.append(f"ROE {r['roe']:.1f}% ({'돈 잘 버는 우량주' if r['roe'] >= 15 else '수익성 양호'})")
            if r["per"] is not None:
                reasons.append(f"PER {r['per']:.1f}배 ({'저평가 구간' if r['per'] <= 15 else '이익 대비 가격'})")
            if r["pbr"] is not None:
                reasons.append(f"PBR {r['pbr']:.2f}배")
            if not reasons:
                reasons.append("재무 지표 없음 (개별 우량주 묶음 권장)")
            r["reasons"] = reasons[:3]

    elif preset_id == "dividend_lowvol":
        p_div = _pct_ranks([r["dividend_yield"] for r in rows], higher_is_better=True)
        p_vol = _pct_ranks([r["vol_60d_pct"] for r in rows], higher_is_better=False)
        p_tr = _pct_ranks([r["ret_12m_pct"] if r["ret_12m_pct"] is not None else r["ret_6m_pct"] for r in rows], higher_is_better=True)
        for i, r in enumerate(rows):
            base = _blend([(p_div[i], 0.50), (p_vol[i], 0.35), (p_tr[i], 0.15)]) or 0.0
            pen = -15.0 if (r["dividend_yield"] is None or r["dividend_yield"] <= 0) else 0.0
            r["score"] = round(max(0.0, min(100.0, base + pen)), 1)
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

    rows.sort(key=lambda r: (r["score"], r.get("ret_6m_pct") or -9999.0), reverse=True)
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
    top_n: int = 15,
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

    candidates = _resolve_candidates(market, universe_id, custom_symbols)
    provider = get_provider(market)
    is_mock = current_app.config["MARKET_PROVIDERS"][market.value] != "toss"

    # Fetch 2.3 years for strategy_fit, 1.3 years otherwise
    lookback_days = int((850 if fit_cls else 480) * 86400)
    fetch_from = int(time.time() - lookback_days)
    d = cost_defaults(market)
    costs = Costs(commission_pct=d["commission_pct"], tax_pct=d["tax_pct"], slippage_pct=d["slippage_pct"])
    capital = d["capital"]

    # Fetch fundamentals up front if needed (1 batch request for US, concurrent for KR)
    fundamentals_map: dict[str, dict[str, Any]] = {}
    fundamentals_source: Optional[str] = None
    if preset["needs_fundamentals"]:
        fundamentals_map = fund_mod.get_fundamentals(market.value, [s for s, _ in candidates], use_mock=is_mock)
        sources = {v.get("source") for v in fundamentals_map.values() if v.get("source") and v.get("source") != "unavailable"}
        fundamentals_source = next(iter(sources), "unavailable")

    t0 = time.time()

    def _scan_one(pair: tuple[str, str]) -> dict[str, Any]:
        sym, name = pair
        warn_labels: list[str] = []
        if not is_mock and market == Market.KR:
            try:
                warn_labels = [w["label"] for w in extras.warnings(market, sym)]
            except Exception:
                warn_labels = []
        if exclude_warnings and warn_labels:
            return {"_excluded_warning": True, "symbol": sym, "warnings": warn_labels}

        try:
            candles = provider.get_history(market, sym, Interval.D1, fetch_from)
            bars = Bars.from_candles(candles)
        except Exception:
            return {"_skipped": True, "symbol": sym}

        if len(bars) < 40 or bars.close[-1] <= 0:
            return {"_skipped": True, "symbol": sym}

        stats = _compute_price_stats(bars)
        f = fundamentals_map.get(sym) or {}
        row: dict[str, Any] = {
            "symbol": sym,
            "name": name,
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
            "warnings": warn_labels,
            "score": 0.0,
            "reasons": [],
        }

        if preset_id == "smart_money":
            if is_mock:
                mf = _mock_flow(sym)
                row.update(flow_5d_for=mf["f5"], flow_5d_inst=mf["i5"], flow_20d_for=mf["f20"],
                           flow_20d_inst=mf["i20"], foreign_delta_pct=mf["f_delta"])
            else:
                try:
                    recs = extras.investor_flow(sym, 20).get("records") or []
                    f5 = sum((r.get("foreigner") or 0) for r in recs[:5])
                    i5 = sum((r.get("institution") or 0) for r in recs[:5])
                    f20 = sum((r.get("foreigner") or 0) for r in recs[:20])
                    i20 = sum((r.get("institution") or 0) for r in recs[:20])
                    h_now = recs[0].get("foreign_holding_pct") if recs else None
                    h_old = recs[-1].get("foreign_holding_pct") if len(recs) >= 2 else None
                    f_delta = round(h_now - h_old, 2) if (h_now is not None and h_old is not None) else None
                    row.update(flow_5d_for=f5, flow_5d_inst=i5, flow_20d_for=f20, flow_20d_inst=i20, foreign_delta_pct=f_delta)
                except Exception:
                    row.update(flow_5d_for=None, flow_5d_inst=None, flow_20d_for=None, flow_20d_inst=None, foreign_delta_pct=None)

        if fit_cls is not None:
            row.update(_run_strategy_fit(bars, fit_cls, capital, costs))

        return row

    workers = 1 if is_mock else min(4, len(candidates))
    if workers == 1:
        raw_rows = [_scan_one(c) for c in candidates]
    else:
        with ThreadPoolExecutor(max_workers=workers) as pool:
            futs = [pool.submit(copy_current_request_context(_scan_one), c) for c in candidates]
            raw_rows = [f.result() for f in futs]

    excluded_warnings = [r["symbol"] for r in raw_rows if r.get("_excluded_warning")]
    skipped = sum(1 for r in raw_rows if r.get("_skipped"))
    valid_rows = [r for r in raw_rows if not r.get("_excluded_warning") and not r.get("_skipped")]

    scored = _score_and_explain(preset_id, valid_rows)
    limit = max(1, min(int(top_n), len(scored))) if scored else 0

    notices: list[str] = []
    if is_mock:
        notices.append("현재 연습용 가상 데이터로 발굴 결과를 보여주고 있습니다. [설정]에서 토스증권 API 키를 연결하면 실제 시장 데이터로 스캔합니다.")
    if preset["needs_fundamentals"] and universe_id in ("kr_etf", "us_etf"):
        notices.append("ETF는 기업 재무제표(PER·PBR·ROE)가 없어 재무 점수가 비어 있을 수 있습니다. 가치·마법공식 발굴은 '대표 우량주' 묶음을 권장합니다.")
    if preset["needs_fundamentals"] and fundamentals_source == "unavailable":
        notices.append("외부 재무 데이터 서버 응답이 지연되어 가격 지표 위주로 계산되었습니다. 잠시 후 다시 시도해 주세요.")

    return {
        "market": market.value,
        "currency": "KRW" if market == Market.KR else "USD",
        "preset": preset_id,
        "universe": universe_id,
        "strategy": fit_cls.id if fit_cls else preset["recommended_strategy"],
        "recommended_portfolio": preset["recommended_portfolio"],
        "scanned": len(candidates),
        "passed": len(scored),
        "skipped": skipped,
        "excluded_warnings": excluded_warnings,
        "fundamentals_source": fundamentals_source,
        "elapsed_sec": round(time.time() - t0, 2),
        "notices": notices,
        "items": scored[:limit],
    }
