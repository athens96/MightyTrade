"""Automated trading: strategy bots (same Strategy classes as the backtester) run on a schedule.

paper mode (default): live data, simulated fills with backtest semantics.
live mode: real Toss orders; enabling needs OTP; per-bot capital cap, user-wide limits and kill switch.
"""
