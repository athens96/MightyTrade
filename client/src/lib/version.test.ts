import { describe, expect, it } from 'vitest'
import { compareSemver, isNewerVersion, parseSemver } from './version'

describe('semantic version comparison', () => {
  it('parses valid x.y.z versions and rejects malformed strings', () => {
    expect(parseSemver('0.1.2')).toEqual([0, 1, 2])
    expect(parseSemver('1.10.3-rc.1')).toEqual([1, 10, 3])
    expect(parseSemver('')).toBeNull()
    expect(parseSemver('v0.1.2')).toBeNull()
  })

  it('detects strictly newer patch, minor, and major versions only', () => {
    expect(isNewerVersion('0.1.3', '0.1.2')).toBe(true)
    expect(isNewerVersion('0.2.0', '0.1.9')).toBe(true)
    expect(isNewerVersion('1.0.0', '0.9.9')).toBe(true)

    expect(isNewerVersion('0.1.2', '0.1.2')).toBe(false)
    expect(isNewerVersion('0.1.1', '0.1.2')).toBe(false)
    expect(isNewerVersion(null, '0.1.2')).toBe(false)
    expect(compareSemver('0.10.0', '0.2.9')).toBe(1)
  })
})
