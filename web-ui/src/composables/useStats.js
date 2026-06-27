import { reactive, onUnmounted } from 'vue'

// Opens a WebSocket to /api/ws and exposes a reactive stats object that the
// dashboard re-renders from. Reconnects automatically on disconnect.
export function useStats() {
  const stats = reactive({
    bytes_in: 0,
    bytes_out: 0,
    total_conns: 0,
    active_conns: 0,
    history: [],
    sessions: []
  })

  let ws = null
  let timer = null
  let stopped = false

  function connect() {
    if (stopped) return
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    ws = new WebSocket(`${proto}://${location.host}/api/ws`)
    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data)
        if (msg.type === 'stats') {
          stats.bytes_in = msg.bytes_in
          stats.bytes_out = msg.bytes_out
          stats.total_conns = msg.total_conns
          stats.active_conns = msg.active_conns
          stats.history = msg.history || []
          stats.sessions = msg.sessions || []
        }
      } catch {
        /* ignore malformed messages */
      }
    }
    ws.onclose = () => {
      if (stopped) return
      timer = setTimeout(connect, 2000)
    }
    ws.onerror = () => {
      ws && ws.close()
    }
  }
  connect()

  onUnmounted(() => {
    stopped = true
    if (timer) clearTimeout(timer)
    ws && ws.close()
  })

  return stats
}
