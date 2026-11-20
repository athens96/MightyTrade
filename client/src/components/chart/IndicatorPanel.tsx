import { useEffect, useRef, useState } from 'react'
import { INDICATORS, defaultParams, type IndicatorConfig, type IndicatorDef, type ParamDef } from '../../indicators/registry'

interface Props {
  value: IndicatorConfig[]
  onChange: (next: IndicatorConfig[]) => void
}

const GROUPS: { group: IndicatorDef['group']; title: string; hint: string }[] = [
  { group: 'overlay', title: '가격 지표', hint: '캔들 위에 겹쳐 표시' },
  { group: 'pane', title: '보조 지표', hint: '차트 아래 별도 영역에 표시' },
]

function ParamInput({ def, value, onChange }: { def: ParamDef; value: number; onChange: (v: number) => void }) {
  const [text, setText] = useState(String(value))
  useEffect(() => setText(String(value)), [value])
  const commit = () => {
    const v = Number(text)
    if (Number.isFinite(v) && v >= def.min && v <= def.max) onChange(v)
    else setText(String(value))
  }
  return (
    <label className="param">
      <span>{def.label}</span>
      <input
        type="number"
        value={text}
        min={def.min}
        max={def.max}
        step={def.step ?? 1}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === 'Enter' && commit()}
      />
    </label>
  )
}

export default function IndicatorPanel({ value, onChange }: Props) {
  const [open, setOpen] = useState(false)
  const [expanded, setExpanded] = useState<string | null>(null)
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const close = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false)
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open])

  const enabled = new Map(value.map((c) => [c.id, c]))

  const toggle = (def: IndicatorDef) => {
    if (enabled.has(def.id)) onChange(value.filter((c) => c.id !== def.id))
    else onChange([...value, { id: def.id, params: defaultParams(def) }])
  }

  const setParam = (id: string, key: string, v: number) =>
    onChange(value.map((c) => (c.id === id ? { ...c, params: { ...c.params, [key]: v } } : c)))

  const reset = (def: IndicatorDef) =>
    onChange(value.map((c) => (c.id === def.id ? { ...c, params: defaultParams(def) } : c)))

  return (
    <div className="indicator-panel" ref={ref}>
      <button className={open ? 'active' : ''} onClick={() => setOpen((o) => !o)}>
        지표 {value.length > 0 && <span className="badge">{value.length}</span>}
      </button>
      {open && (
        <div className="indicator-dropdown">
          {GROUPS.map(({ group, title, hint }) => (
            <section key={group}>
              <h4>
                {title} <span className="muted small">{hint}</span>
              </h4>
              {INDICATORS.filter((d) => d.group === group).map((def) => {
                const cfg = enabled.get(def.id)
                const isOpen = expanded === def.id
                return (
                  <div key={def.id} className={`indicator-item${cfg ? ' on' : ''}`}>
                    <div className="indicator-head">
                      <label>
                        <input type="checkbox" checked={!!cfg} onChange={() => toggle(def)} />
                        {def.name}
                      </label>
                      <button className="link" onClick={() => setExpanded(isOpen ? null : def.id)} title="설명 / 설정">
                        {isOpen ? '접기' : cfg && def.params.length ? '설정' : '설명'}
                      </button>
                    </div>
                    {isOpen && (
                      <div className="indicator-body">
                        <p className="muted small">{def.description}</p>
                        {cfg && def.params.length > 0 && (
                          <div className="params">
                            {def.params.map((p) => (
                              <ParamInput key={p.key} def={p} value={cfg.params[p.key]} onChange={(v) => setParam(def.id, p.key, v)} />
                            ))}
                            <button className="link" onClick={() => reset(def)}>기본값</button>
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )
              })}
            </section>
          ))}
        </div>
      )}
    </div>
  )
}
