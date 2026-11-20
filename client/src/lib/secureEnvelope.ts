// Application-layer encryption for secrets sent to the server (on top of HTTPS).
// Mirrors server/app/transport_crypto.py: RSA-OAEP-256 wraps a one-time AES-256-GCM key;
// AES-GCM additional data = the logged-in user's id, so the envelope only works for that account.

export interface TransportKey {
  kid: string
  alg: string
  spki: string // base64 DER SubjectPublicKeyInfo
}

export interface Envelope {
  kid: string
  alg: string
  ek: string
  iv: string
  ct: string
}

const enc = new TextEncoder()
const b64 = (buf: ArrayBuffer | Uint8Array) => {
  const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf)
  let s = ''
  for (const b of bytes) s += String.fromCharCode(b)
  return btoa(s)
}
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0))

export async function sealWithKey(key: TransportKey, payload: Record<string, unknown>, aad: string): Promise<Envelope> {
  const subtle = globalThis.crypto?.subtle
  if (!subtle) throw new Error('암호화를 지원하지 않는 환경입니다. HTTPS(또는 localhost)로 접속하세요.')

  const pub = await subtle.importKey('spki', unb64(key.spki), { name: 'RSA-OAEP', hash: 'SHA-256' }, false, ['encrypt'])
  const aes = await subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt'])
  const iv = globalThis.crypto.getRandomValues(new Uint8Array(12))
  const data = enc.encode(JSON.stringify({ ...payload, iat: Math.floor(Date.now() / 1000) }))

  const ct = await subtle.encrypt({ name: 'AES-GCM', iv, additionalData: enc.encode(aad) }, aes, data)
  const ek = await subtle.encrypt({ name: 'RSA-OAEP' }, pub, await subtle.exportKey('raw', aes))
  return { kid: key.kid, alg: key.alg, ek: b64(ek), iv: b64(iv), ct: b64(ct) }
}
