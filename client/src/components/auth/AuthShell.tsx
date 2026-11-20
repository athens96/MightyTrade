import { useEffect, useMemo, useState, type ReactNode } from 'react'

/**
 * Backdrop for the signed-out screens (login, OTP): a slowly scrolling candlestick chart and a
 * tape of clocks / session hours. The chart is a seeded random walk — purely decorative. No market
 * data is shown before login (Toss data may not be redistributed).
 */
export default function AuthShell({ children }: { children: ReactNode }) {
  const chart = useMemo(() => candlesSvg(80, 11), [])
  const now = useNow(15_000)
  const tape = tapeItems(now)

  return (
    <div className="auth-page">
      <div className="auth-tape" aria-hidden>
        <div className="auth-tape-track">
          {[0, 1].map((copy) => (
            <span key={copy} className="auth-tape-set">
              {tape.map((t, i) => (
                <span key={i}>{t.label && <b>{t.label}</b>}{t.value}</span>
              ))}
            </span>
          ))}
        </div>
      </div>
      <div className="auth-bg" aria-hidden>
        <div className="auth-grid" />
        <div className="auth-chart">
          <div dangerouslySetInnerHTML={{ __html: chart }} />
          <div dangerouslySetInnerHTML={{ __html: chart }} />
        </div>
      </div>
      <main className="auth-wrap">
        <div className="auth-card">{children}</div>
      </main>
      <footer className="auth-foot">투자 판단과 그 결과의 책임은 이용자 본인에게 있습니다</footer>
    </div>
  )
}

function useNow(everyMs: number) {
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const t = window.setInterval(() => setNow(new Date()), everyMs)
    return () => window.clearInterval(t)
  }, [everyMs])
  return now
}

const hm = (d: Date, timeZone: string) =>
  d.toLocaleTimeString('ko-KR', { timeZone, hour: '2-digit', minute: '2-digit', hour12: false })

function tapeItems(now: Date) {
  // US regular session 09:30–16:00 New York time, shown in KST (13h ahead in summer, 14h in winter)
  const hour = (tz: string) => Number(now.toLocaleString('en-US', { timeZone: tz, hour: 'numeric', hour12: false })) % 24
  const offset = (hour('Asia/Seoul') - hour('America/New_York') + 24) % 24
  const usOpen = `${String((9 + offset) % 24).padStart(2, '0')}:30`
  const usClose = `${String((16 + offset) % 24).padStart(2, '0')}:00`
  return [
    { label: '서울', value: hm(now, 'Asia/Seoul') },
    { label: '뉴욕', value: hm(now, 'America/New_York') },
    { label: '국내 정규장', value: '09:00–15:30' },
    { label: '미국 정규장', value: `${usOpen}–${usClose} (KST)` },
    { label: '', value: '실시간 시세·호가' },
    { label: '', value: '퀀트 랩 백테스트' },
    { label: '', value: '자동매매 봇' },
    { label: '', value: 'OTP 2단계 인증' },
  ]
}

function candlesSvg(n: number, seed: number) {
  let s = seed
  const rnd = () => ((s = (s * 16807) % 2147483647) / 2147483647)
  const bars: { o: number; h: number; l: number; c: number }[] = []
  let p = 100
  for (let i = 0; i < n; i++) {
    const o = p
    const c = o * (1 + 0.0015 + (rnd() - 0.5) * 0.036)
    bars.push({ o, c, h: Math.max(o, c) * (1 + rnd() * 0.011), l: Math.min(o, c) * (1 - rnd() * 0.011) })
    p = c
  }
  const W = 1600, H = 600
  const hi = Math.max(...bars.map((b) => b.h)), lo = Math.min(...bars.map((b) => b.l))
  const w = W / n
  const y = (v: number) => (H - ((v - lo) / (hi - lo)) * H * 0.86 - H * 0.07).toFixed(1)
  let out = `<svg viewBox="0 0 ${W} ${H}" preserveAspectRatio="none">`
  bars.forEach((b, i) => {
    const x = (i * w + w / 2).toFixed(1)
    const col = b.c >= b.o ? '#e5484d' : '#3e63dd'
    const top = Number(y(Math.max(b.o, b.c)))
    const hgt = Math.max(1.5, Math.abs(Number(y(b.o)) - Number(y(b.c))))
    out += `<line x1="${x}" x2="${x}" y1="${y(b.h)}" y2="${y(b.l)}" stroke="${col}" stroke-width="1.4"/>`
    out += `<rect x="${(i * w + w * 0.19).toFixed(1)}" y="${top}" width="${(w * 0.62).toFixed(1)}" height="${hgt.toFixed(1)}" fill="${col}" rx="1"/>`
  })
  const ma = bars.map((_, i) => {
    const a = bars.slice(Math.max(0, i - 11), i + 1)
    return `${(i * w + w / 2).toFixed(1)},${y(a.reduce((t, b) => t + b.c, 0) / a.length)}`
  })
  return out + `<polyline points="${ma.join(' ')}" fill="none" stroke="#ffd54f" stroke-width="2.5" stroke-linejoin="round"/></svg>`
}
