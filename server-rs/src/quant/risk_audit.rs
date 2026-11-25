//! 6-Gate Risk Manager automatic audit for backtests & portfolios (MT-25).
//!
//! Inspired by the risk-manager review gates in the *151 Trading Strategies* workflow:
//! evaluates every backtest from an adversarial risk-manager perspective to flag
//! insufficient sample size, lucky-trade concentration, cost-eroded expectancy,
//! weak risk-adjusted returns, unsustainable drawdown, and benchmark underperformance.

use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateStatus {
    Pass,
    Warn,
    Fail,
}

impl GateStatus {
    fn as_str(self) -> &'static str {
        match self {
            GateStatus::Pass => "pass",
            GateStatus::Warn => "warn",
            GateStatus::Fail => "fail",
        }
    }
}

struct Gate {
    id: &'static str,
    title: &'static str,
    status: GateStatus,
    value: String,
    detail: String,
}

fn finalize_audit(gates: Vec<Gate>) -> Value {
    let passed = gates.iter().filter(|g| g.status == GateStatus::Pass).count();
    let warned = gates.iter().filter(|g| g.status == GateStatus::Warn).count();
    let failed = gates.iter().filter(|g| g.status == GateStatus::Fail).count();

    let (verdict, verdict_label, summary) = if failed == 0 && warned <= 1 {
        (
            "PASS",
            "KEEP · 실전 후보 통과",
            format!(
                "6개 리스크 게이트 중 {passed}개 통과 · {warned}개 주의 — 모의투자(Paper)로 실전 점검을 시작해 볼 만한 후보입니다."
            ),
        )
    } else if failed == 0 || (failed == 1 && passed >= 3) {
        (
            "CAUTION",
            "주의 · 조건 보완 권장",
            format!(
                "6개 중 {passed}개 통과 · {warned}개 주의 · {failed}개 탈락 — 파라미터 최적화·워크포워드나 공통 손절 보호막을 추가해 보완하세요."
            ),
        )
    } else {
        (
            "FAIL",
            "KILL · 실전 투입 보류",
            format!(
                "6개 중 {failed}개 게이트 탈락 ({passed}개 통과 · {warned}개 주의) — 현재 설정으로는 실거래 투입을 보류하고 전략·종목을 재검토하세요."
            ),
        )
    };

    let gate_json: Vec<Value> = gates
        .into_iter()
        .map(|g| {
            json!({
                "id": g.id,
                "title": g.title,
                "status": g.status.as_str(),
                "value": g.value,
                "detail": g.detail,
            })
        })
        .collect();

    json!({
        "verdict": verdict,
        "verdict_label": verdict_label,
        "summary": summary,
        "passed": passed,
        "warned": warned,
        "failed": failed,
        "gates": gate_json,
    })
}

/// Evaluates a single-symbol strategy backtest across 6 adversarial risk gates.
pub fn audit_backtest(
    metrics: &Value,
    benchmark: &Value,
    trades: &[Value],
    timeframe: &str,
) -> Value {
    let closed: Vec<&Value> = trades
        .iter()
        .filter(|t| !t.get("open").and_then(|v| v.as_bool()).unwrap_or(true))
        .collect();
    let n_trades = closed.len();
    let min_pass = if timeframe == "1d" { 15 } else { 20 };

    // Gate 1: Sample size
    let g1 = if n_trades >= min_pass {
        Gate {
            id: "sample_size",
            title: "표본 충분성 (거래 횟수)",
            status: GateStatus::Pass,
            value: format!("{n_trades}회"),
            detail: format!("통계적으로 판단할 수 있는 충분한 거래 횟수입니다 (기준 {min_pass}회 이상)."),
        }
    } else if n_trades >= 5 {
        Gate {
            id: "sample_size",
            title: "표본 충분성 (거래 횟수)",
            status: GateStatus::Warn,
            value: format!("{n_trades}회"),
            detail: format!("거래 횟수가 다소 적습니다(권장 {min_pass}회 이상). 기간을 늘려 재확인하세요."),
        }
    } else {
        Gate {
            id: "sample_size",
            title: "표본 충분성 (거래 횟수)",
            status: GateStatus::Fail,
            value: format!("{n_trades}회"),
            detail: "거래 횟수가 5회 미만이라 우연일 가능성이 매우 높습니다.".to_string(),
        }
    };

    // Gate 2: Concentration in top 1-2 lucky trades
    let pnls: Vec<f64> = closed
        .iter()
        .filter_map(|t| t.get("pnl").and_then(|v| v.as_f64()))
        .collect();
    let net_pnl: f64 = pnls.iter().sum();
    let mut wins: Vec<f64> = pnls.iter().copied().filter(|&x| x > 0.0).collect();
    wins.sort_by(|a, b| b.total_cmp(a));
    let gross_win: f64 = wins.iter().sum();

    let g2 = if n_trades == 0 || gross_win <= 0.0 || net_pnl <= 0.0 {
        Gate {
            id: "concentration",
            title: "수익 분산도 (소수 거래 편중)",
            status: GateStatus::Fail,
            value: if n_trades == 0 { "거래 없음".to_string() } else { "누적 손실".to_string() },
            detail: "확정 거래 기준 누적 순수익이 플러스여야 수익 분산도를 통과할 수 있습니다.".to_string(),
        }
    } else {
        let top1_pct = wins[0] / gross_win * 100.0;
        if top1_pct < 40.0 && wins.len() >= 3 {
            Gate {
                id: "concentration",
                title: "수익 분산도 (소수 거래 편중)",
                status: GateStatus::Pass,
                value: format!("최대 1건 비중 {top1_pct:.0}%"),
                detail: "수익이 한두 번의 대박에 쏠리지 않고 여러 거래에 고르게 분산되어 있습니다.".to_string(),
            }
        } else if top1_pct < 65.0 && wins.len() >= 2 {
            Gate {
                id: "concentration",
                title: "수익 분산도 (소수 거래 편중)",
                status: GateStatus::Warn,
                value: format!("최대 1건 비중 {top1_pct:.0}%"),
                detail: "상위 1건 거래 의존도가 다소 높습니다. 몬테카를로 검증으로 운 요소를 확인하세요.".to_string(),
            }
        } else {
            Gate {
                id: "concentration",
                title: "수익 분산도 (소수 거래 편중)",
                status: GateStatus::Fail,
                value: format!("최대 1건 비중 {top1_pct:.0}%"),
                detail: "전체 이익의 대부분이 단 1건의 운 좋은 거래에서 나와 재현성이 낮습니다.".to_string(),
            }
        }
    };

    // Gate 3: Cost-adjusted expectancy & profit factor
    let pf = metrics.get("profit_factor").and_then(|v| v.as_f64());
    let avg_ret = metrics.get("avg_return_pct").and_then(|v| v.as_f64());
    let g3 = match (pf, avg_ret) {
        (Some(p), Some(a)) if p >= 1.25 && a > 0.0 => Gate {
            id: "cost_edge",
            title: "비용 차감 후 기대값 (손익비)",
            status: GateStatus::Pass,
            value: format!("PF {p:.2} · 평균 {a:+.2}%"),
            detail: "수수료·세금·슬리피지를 모두 빼고도 거래당 기대수익과 손익비가 안정적입니다.".to_string(),
        },
        (Some(p), Some(a)) if p >= 1.0 && a > 0.0 => Gate {
            id: "cost_edge",
            title: "비용 차감 후 기대값 (손익비)",
            status: GateStatus::Warn,
            value: format!("PF {p:.2} · 평균 {a:+.2}%"),
            detail: "비용 후 수익은 플러스지만 여유 폭이 얇아 실전 슬리피지에 취약할 수 있습니다.".to_string(),
        },
        (None, Some(a)) if a > 0.0 && n_trades >= 3 => Gate {
            id: "cost_edge",
            title: "비용 차감 후 기대값 (손익비)",
            status: GateStatus::Pass,
            value: format!("손실 0건 · 평균 {a:+.2}%"),
            detail: "비용 차감 후 모든 확정 거래가 수익을 기록했습니다.".to_string(),
        },
        _ => Gate {
            id: "cost_edge",
            title: "비용 차감 후 기대값 (손익비)",
            status: GateStatus::Fail,
            value: match (pf, avg_ret) {
                (Some(p), Some(a)) => format!("PF {p:.2} · 평균 {a:+.2}%"),
                _ => "기대값 부족".to_string(),
            },
            detail: "수수료·세금·슬리피지를 차감하면 수익 우위가 사라지거나 손실이 발생합니다.".to_string(),
        },
    };

    // Gate 4: Sharpe ratio
    let sharpe = metrics.get("sharpe").and_then(|v| v.as_f64());
    let g4 = match sharpe {
        Some(s) if s >= 0.8 => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Pass,
            value: format!("{s:.2}"),
            detail: "감수한 변동성 대비 우수한 위험조정 수익률입니다 (기준 0.8 이상).".to_string(),
        },
        Some(s) if s >= 0.3 => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Warn,
            value: format!("{s:.2}"),
            detail: "보통 수준의 위험 대비 수익률입니다 (0.8 이상이면 안정적).".to_string(),
        },
        Some(s) => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Fail,
            value: format!("{s:.2}"),
            detail: "변동성에 비해 수익이 부족합니다(0.3 미만). 다른 전략이나 필터를 검토하세요.".to_string(),
        },
        None => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Fail,
            value: "-".to_string(),
            detail: "거래나 수익 변동이 거의 없어 샤프 비율을 산출할 수 없습니다.".to_string(),
        },
    };

    // Gate 5: Max drawdown survivability
    let mdd = metrics.get("mdd_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let g5 = if mdd >= -20.0 {
        Gate {
            id: "drawdown",
            title: "낙폭 생존성 (최대 낙폭 MDD)",
            status: GateStatus::Pass,
            value: format!("{mdd:.1}%"),
            detail: "최대 낙폭이 -20% 이내로 실전 운용 시 심리적으로 버틸 수 있는 수준입니다.".to_string(),
        }
    } else if mdd >= -35.0 {
        Gate {
            id: "drawdown",
            title: "낙폭 생존성 (최대 낙폭 MDD)",
            status: GateStatus::Warn,
            value: format!("{mdd:.1}%"),
            detail: "낙폭이 다소 큽니다(-20% ~ -35%). 마켓 타이밍 필터나 손절 보호막을 켜 보세요.".to_string(),
        }
    } else {
        Gate {
            id: "drawdown",
            title: "낙폭 생존성 (최대 낙폭 MDD)",
            status: GateStatus::Fail,
            value: format!("{mdd:.1}%"),
            detail: "최대 낙폭이 -35%를 초과해 실전에서 계좌를 끝까지 유지하기 어렵습니다.".to_string(),
        }
    };

    // Gate 6: Advantage vs Buy & Hold
    let ret = metrics.get("total_return_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b_ret = benchmark.get("total_return_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b_sharpe = benchmark.get("sharpe").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b_mdd = benchmark.get("mdd_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let beats_sharpe = sharpe.is_some_and(|s| s > b_sharpe + 0.05 && s > 0.0);
    let beats_ret = ret > b_ret && ret > 0.0;
    let cuts_mdd = ret > 0.0 && b_mdd < -5.0 && mdd.abs() <= b_mdd.abs() * 0.70;

    let g6 = if beats_sharpe || beats_ret || cuts_mdd {
        Gate {
            id: "vs_benchmark",
            title: "단순 보유(B&H) 대비 실전 우위",
            status: GateStatus::Pass,
            value: format!("전략 {ret:+.1}% vs 보유 {b_ret:+.1}%"),
            detail: "그냥 사서 들고 있는 것보다 수익률·샤프·낙폭 방어 중 뚜렷한 우위가 확인됩니다.".to_string(),
        }
    } else if ret > 0.0 && mdd > b_mdd {
        Gate {
            id: "vs_benchmark",
            title: "단순 보유(B&H) 대비 실전 우위",
            status: GateStatus::Warn,
            value: format!("전략 {ret:+.1}% vs 보유 {b_ret:+.1}%"),
            detail: "수익률은 단순 보유보다 낮지만 하락장 낙폭을 일부 줄여 방어 성격이 있습니다.".to_string(),
        }
    } else {
        Gate {
            id: "vs_benchmark",
            title: "단순 보유(B&H) 대비 실전 우위",
            status: GateStatus::Fail,
            value: format!("전략 {ret:+.1}% vs 보유 {b_ret:+.1}%"),
            detail: "수익률과 위험 모두 단순 보유 대비 이점이 부족합니다.".to_string(),
        }
    };

    finalize_audit(vec![g1, g2, g3, g4, g5, g6])
}

/// Evaluates a multi-asset portfolio backtest across 6 risk gates.
pub fn audit_portfolio(
    metrics: &Value,
    benchmark: &Value,
    rebalances: &[Value],
    equity: &[Value],
) -> Value {
    let n_rebal = rebalances.len();

    // Gate 1: Rebalance sample count
    let g1 = if n_rebal >= 6 {
        Gate {
            id: "sample_size",
            title: "리밸런싱 표본 충분성",
            status: GateStatus::Pass,
            value: format!("{n_rebal}회"),
            detail: "여러 시장 국면에 걸쳐 충분히 리밸런싱이 검증되었습니다 (6회 이상).".to_string(),
        }
    } else if n_rebal >= 3 {
        Gate {
            id: "sample_size",
            title: "리밸런싱 표본 충분성",
            status: GateStatus::Warn,
            value: format!("{n_rebal}회"),
            detail: "리밸런싱 횟수가 다소 적습니다. 기간을 3~5년 이상으로 넓혀 확인하세요.".to_string(),
        }
    } else {
        Gate {
            id: "sample_size",
            title: "리밸런싱 표본 충분성",
            status: GateStatus::Fail,
            value: format!("{n_rebal}회"),
            detail: "리밸런싱 횟수가 3회 미만이라 장기 자산배분 효과를 판단하기 어렵습니다.".to_string(),
        }
    };

    // Gate 2: Consistency across periods (positive ~21-trading-day blocks)
    let eq_vals: Vec<f64> = equity
        .iter()
        .filter_map(|p| p.get("equity").and_then(|v| v.as_f64()))
        .collect();
    let mut pos_months = 0usize;
    let mut tot_months = 0usize;
    let step = 21usize;
    let mut idx = step;
    while idx < eq_vals.len() {
        let prev = eq_vals[idx - step];
        let cur = eq_vals[idx];
        if prev > 0.0 {
            tot_months += 1;
            if cur >= prev {
                pos_months += 1;
            }
        }
        idx += step;
    }
    let pos_ratio = if tot_months > 0 {
        pos_months as f64 / tot_months as f64 * 100.0
    } else {
        0.0
    };
    let g2 = if tot_months >= 4 && pos_ratio >= 55.0 {
        Gate {
            id: "consistency",
            title: "월간 수익 일관성",
            status: GateStatus::Pass,
            value: format!("상승월 {pos_ratio:.0}% ({pos_months}/{tot_months})"),
            detail: "특정 한 달에 몰리지 않고 꾸준히 우상향하는 안정적인 수익 곡선입니다.".to_string(),
        }
    } else if tot_months >= 2 && pos_ratio >= 45.0 {
        Gate {
            id: "consistency",
            title: "월간 수익 일관성",
            status: GateStatus::Warn,
            value: format!("상승월 {pos_ratio:.0}% ({pos_months}/{tot_months})"),
            detail: "월간 등락 편차가 보통 수준입니다.".to_string(),
        }
    } else {
        Gate {
            id: "consistency",
            title: "월간 수익 일관성",
            status: GateStatus::Fail,
            value: format!("상승월 {pos_ratio:.0}%"),
            detail: "하락월 비중이 높아 장기 보유 시 심리적 흔들림이 클 수 있습니다.".to_string(),
        }
    };

    // Gate 3: Turnover / cost efficiency
    let avg_turn = if n_rebal > 0 {
        rebalances
            .iter()
            .filter_map(|r| r.get("turnover_pct").and_then(|v| v.as_f64()))
            .sum::<f64>()
            / n_rebal as f64
    } else {
        0.0
    };
    let cagr = metrics.get("cagr_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let g3 = if cagr > 0.0 && avg_turn <= 70.0 {
        Gate {
            id: "turnover",
            title: "회전율 · 거래비용 효율",
            status: GateStatus::Pass,
            value: format!("평균 회전율 {avg_turn:.1}% · 연 {cagr:+.1}%"),
            detail: "리밸런싱 회전율이 적정하고 비용 차감 후 연복리 수익률도 플러스입니다.".to_string(),
        }
    } else if cagr > 0.0 {
        Gate {
            id: "turnover",
            title: "회전율 · 거래비용 효율",
            status: GateStatus::Warn,
            value: format!("평균 회전율 {avg_turn:.1}% · 연 {cagr:+.1}%"),
            detail: "수익은 플러스지만 회전율이 높아 수수료·세금이 누적될 수 있습니다.".to_string(),
        }
    } else {
        Gate {
            id: "turnover",
            title: "회전율 · 거래비용 효율",
            status: GateStatus::Fail,
            value: format!("평균 회전율 {avg_turn:.1}% · 연 {cagr:+.1}%"),
            detail: "비용 반영 후 연복리 수익률이 마이너스입니다.".to_string(),
        }
    };

    // Gate 4: Sharpe
    let sharpe = metrics.get("sharpe").and_then(|v| v.as_f64());
    let g4 = match sharpe {
        Some(s) if s >= 0.8 => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Pass,
            value: format!("{s:.2}"),
            detail: "분산 투자 효과가 잘 작동해 위험 대비 수익률이 우수합니다.".to_string(),
        },
        Some(s) if s >= 0.3 => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Warn,
            value: format!("{s:.2}"),
            detail: "위험 대비 수익률이 무난한 편입니다.".to_string(),
        },
        _ => Gate {
            id: "sharpe",
            title: "위험 대비 수익 (샤프 비율)",
            status: GateStatus::Fail,
            value: sharpe.map(|s| format!("{s:.2}")).unwrap_or_else(|| "-".to_string()),
            detail: "변동성 대비 수익이 부족합니다(0.3 미만). 자산 구성을 점검하세요.".to_string(),
        },
    };

    // Gate 5: MDD
    let mdd = metrics.get("mdd_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let g5 = if mdd >= -20.0 {
        Gate {
            id: "drawdown",
            title: "낙폭 생존성 (최대 낙폭 MDD)",
            status: GateStatus::Pass,
            value: format!("{mdd:.1}%"),
            detail: "최대 낙폭이 -20% 이내로 장기 자산배분에 적합합니다.".to_string(),
        }
    } else if mdd >= -30.0 {
        Gate {
            id: "drawdown",
            title: "낙폭 생존성 (최대 낙폭 MDD)",
            status: GateStatus::Warn,
            value: format!("{mdd:.1}%"),
            detail: "낙폭이 -20% ~ -30% 구간입니다. 채권·금 등 방어 자산 비중을 검토하세요.".to_string(),
        }
    } else {
        Gate {
            id: "drawdown",
            title: "낙폭 생존성 (최대 낙폭 MDD)",
            status: GateStatus::Fail,
            value: format!("{mdd:.1}%"),
            detail: "최대 낙폭이 -30%를 넘어 분산 투자 방어력이 부족합니다.".to_string(),
        }
    };

    // Gate 6: vs equal-weight Buy & Hold
    let ret = metrics.get("total_return_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b_ret = benchmark.get("total_return_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b_sharpe = benchmark.get("sharpe").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b_mdd = benchmark.get("mdd_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let g6 = if sharpe.is_some_and(|s| s > b_sharpe + 0.05 && s > 0.0)
        || (ret > b_ret && ret > 0.0)
        || (ret > 0.0 && b_mdd < -5.0 && mdd.abs() <= b_mdd.abs() * 0.75)
    {
        Gate {
            id: "vs_benchmark",
            title: "동일비중 단순보유 대비 우위",
            status: GateStatus::Pass,
            value: format!("모델 {ret:+.1}% vs 보유 {b_ret:+.1}%"),
            detail: "단순 보유보다 수익·샤프·낙폭 방어 중 뚜렷한 장점이 확인됩니다.".to_string(),
        }
    } else if ret > 0.0 && mdd > b_mdd {
        Gate {
            id: "vs_benchmark",
            title: "동일비중 단순보유 대비 우위",
            status: GateStatus::Warn,
            value: format!("모델 {ret:+.1}% vs 보유 {b_ret:+.1}%"),
            detail: "수익률은 낮지만 하락장 낙폭을 줄여주는 효과가 있습니다.".to_string(),
        }
    } else {
        Gate {
            id: "vs_benchmark",
            title: "동일비중 단순보유 대비 우위",
            status: GateStatus::Fail,
            value: format!("모델 {ret:+.1}% vs 보유 {b_ret:+.1}%"),
            detail: "단순 보유 대비 뚜렷한 개선 효과가 나타나지 않았습니다.".to_string(),
        }
    };

    finalize_audit(vec![g1, g2, g3, g4, g5, g6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_backtest_passes_healthy_strategy_and_fails_concentrated_loser() {
        let good_trades: Vec<Value> = (0..20)
            .map(|i| {
                let win = i % 3 != 0;
                json!({
                    "open": false,
                    "pnl": if win { 120_000.0 } else { -60_000.0 },
                    "return_pct": if win { 2.4 } else { -1.2 },
                })
            })
            .collect();
        let good_m = json!({
            "total_return_pct": 38.0,
            "sharpe": 1.35,
            "mdd_pct": -12.5,
            "profit_factor": 2.4,
            "avg_return_pct": 1.2,
        });
        let bench = json!({
            "total_return_pct": 20.0,
            "sharpe": 0.7,
            "mdd_pct": -28.0,
        });
        let pass_res = audit_backtest(&good_m, &bench, &good_trades, "1d");
        assert_eq!(pass_res["verdict"], "PASS");
        assert_eq!(pass_res["passed"], 6);

        // Single lucky trade + terrible drawdown + negative expectancy -> FAIL
        let bad_trades = vec![
            json!({"open": false, "pnl": 100_000.0, "return_pct": 10.0}),
            json!({"open": false, "pnl": -150_000.0, "return_pct": -15.0}),
        ];
        let bad_m = json!({
            "total_return_pct": -12.0,
            "sharpe": -0.4,
            "mdd_pct": -42.0,
            "profit_factor": 0.67,
            "avg_return_pct": -2.5,
        });
        let fail_res = audit_backtest(&bad_m, &bench, &bad_trades, "1d");
        assert_eq!(fail_res["verdict"], "FAIL");
        assert!(fail_res["failed"].as_u64().unwrap() >= 4);
    }
}
