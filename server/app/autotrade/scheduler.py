"""Background scheduler for bots. Exactly one per server: guarded by an exclusive file lock.

Each tick (30s), per user with enabled bots and per market:
- inside a regular session: run each bot once per trading day, 1 minute after the open
  (not in the last hour before the close — a late start skips the day instead of trading blind)
- live bots with open orders: reconcile fills every 2 minutes
"""
from __future__ import annotations

import fcntl
import logging
import multiprocessing
import os
import threading
from datetime import datetime, timedelta, timezone
from typing import Optional

from flask import Flask, g

from ..errors import ApiError
from ..market import extras
from ..users import credentials
from ..users import repository as users
from ..users.repository import now_iso
from . import bots, executor

log = logging.getLogger(__name__)

TICK_SEC = 30
OPEN_DELAY = timedelta(minutes=1)
LATE_CUTOFF = timedelta(hours=1)
RECONCILE_EVERY = timedelta(minutes=2)
ERROR_BACKOFF = timedelta(minutes=10)


def sessions(market: str) -> list:
    """[(trading_date, start, end)] regular sessions for previous/today/next business day."""
    cal = extras.calendar(market)
    out = []
    for key in ("previousBusinessDay", "today", "nextBusinessDay"):
        day = cal.get(key) or {}
        block = day.get("integrated") if market == "KR" else day
        reg = block.get("regularMarket") if isinstance(block, dict) else None
        if isinstance(reg, dict):
            out.append((day.get("date"), extras._ts(reg["startTime"]), extras._ts(reg["endTime"])))
    return out


def active_session(market: str, now: datetime) -> Optional[tuple]:
    return next(((d, s, e) for d, s, e in sessions(market) if s <= now < e), None)


def target_date(market: str, now: datetime) -> str:
    """Trading day a decision made now would act on (the current session, else the next one)."""
    for d, s, e in sessions(market):
        if now < e:
            return d
    return (extras.calendar(market).get("nextBusinessDay") or {}).get("date") or now.date().isoformat()


def _recent(iso: Optional[str], within: timedelta, now: datetime) -> bool:
    return bool(iso) and now - datetime.fromisoformat(iso) < within


def tick(app: Flask, now: Optional[datetime] = None) -> None:
    now = now or datetime.now(timezone.utc)
    with app.app_context():
        by_user: dict = {}
        for b in bots.all_enabled():
            by_user.setdefault(b["user_id"], []).append(b)
    for uid, user_bots in by_user.items():
        with app.app_context():
            user = users.get_user(uid)
            if not user or user.get("autotrade_halted") or not credentials.get_toss(uid):
                continue
            g.user = user
            for market in sorted({b["market"] for b in user_bots}):
                try:
                    sess = active_session(market, now)
                except ApiError as e:
                    log.warning("calendar failed for %s/%s: %s", uid[:8], market, e.message)
                    continue
                if not sess:
                    continue
                date, start, end = sess
                for bot in (b for b in user_bots if b["market"] == market):
                    _step(bot, date, start, end, now)


def _step(bot: dict, date: str, start: datetime, end: datetime, now: datetime) -> None:
    st = bots.state(bot)
    try:
        if bot["mode"] == "live" and st["pending"] and not _recent(st.get("last_reconcile"), RECONCILE_EVERY, now):
            executor.reconcile_live(bot, date)
            bots.save(bot)
        if st["last_run_date"] == date or now < start + OPEN_DELAY:
            return
        if _recent((st.get("last_error") or {}).get("at"), ERROR_BACKOFF, now):
            return
        if now > end - LATE_CUTOFF:
            st["last_run_date"] = date
            bots.save(bot)
            bots.log(bot, "skip", "장 마감 1시간 전 이후라 오늘 실행을 건너뜁니다", "warn")
            return
        executor.run(bot, date)
        st.pop("last_error", None)
    except ApiError as e:
        st["last_error"] = {"at": now_iso(), "message": e.message}
        bots.save(bot)
        bots.log(bot, "error", f"실행 실패: {e.message} (10분 뒤 재시도)", "error", {"code": e.code})
    except Exception as e:  # noqa: BLE001 - one broken bot must not stop the others
        log.exception("bot %s failed", bot["id"])
        st["last_error"] = {"at": now_iso(), "message": str(e)[:200]}
        bots.save(bot)
        bots.log(bot, "error", f"내부 오류: {str(e)[:200]}", "error")


class Scheduler:
    def __init__(self, app: Flask):
        self.app = app
        self._stop = threading.Event()
        self._lock_file = None
        self.running = False
        self.last_tick: Optional[str] = None
        self.last_error: Optional[str] = None

    def start(self) -> bool:
        path = os.path.join(str(self.app.config["DATA_DIR"]), ".autotrade.lock")
        f = open(path, "a")
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)  # another process already runs the scheduler
        except BlockingIOError:
            f.close()
            return False
        self._lock_file = f
        self.running = True
        threading.Thread(target=self._loop, name="autotrade-scheduler", daemon=True).start()
        log.info("autotrade scheduler started (pid %s)", os.getpid())
        return True

    def stop(self) -> None:
        self._stop.set()

    def _loop(self) -> None:
        while not self._stop.is_set():
            try:
                tick(self.app)
                self.last_error = None
            except Exception as e:  # noqa: BLE001
                log.exception("scheduler tick failed")
                self.last_error = str(e)[:200]
            self.last_tick = now_iso()
            self._stop.wait(TICK_SEC)


def maybe_start(app: Flask) -> None:
    """Start in the serving process only (not in the Werkzeug reloader's watcher or spawned workers)."""
    if not app.config.get("AUTOTRADE_SCHEDULER") or app.testing:
        return
    if multiprocessing.current_process().name != "MainProcess":
        return
    if app.debug and os.environ.get("WERKZEUG_RUN_MAIN") != "true":
        return
    sched = Scheduler(app)
    app.extensions["autotrade_scheduler"] = sched if sched.start() else None
