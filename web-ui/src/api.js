// Tiny fetch wrapper with cookie-based auth. All requests send credentials so
// the HttpOnly JWT cookie is included automatically.
async function request(url, options = {}) {
  const res = await fetch(url, {
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', ...(options.headers || {}) },
    ...options,
    body: options.body ? JSON.stringify(options.body) : undefined
  })
  const text = await res.text()
  const data = text ? JSON.parse(text) : null
  if (!res.ok) {
    throw new Error((data && data.error) || `${res.status} ${res.statusText}`)
  }
  return data
}

export const api = {
  get: (url) => request(url, { method: 'GET' }),
  post: (url, body) => request(url, { method: 'POST', body }),
  patch: (url, body) => request(url, { method: 'PATCH', body }),
  del: (url) => request(url, { method: 'DELETE' })
}
